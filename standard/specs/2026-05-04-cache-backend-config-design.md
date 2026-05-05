# Design: `BackendConfig` — timeout, retry, max-artifact knobs

**Date:** 2026-05-04
**Status:** Design — pending implementation plan
**Standard change ID:** CS-0057
**Linear epic:** SHI-24 cache cloud-readiness — *user-overrideable backend tunables*
**Predecessors:**
  - [2026-05-01-cache-cloud-readiness-design.md](./2026-05-01-cache-cloud-readiness-design.md) (introduced the `CacheBackend` trait, `cloud_key`, and the local-only artifact store)
  - [2026-05-04-cache-cloud-grade-integrity-design.md](./2026-05-04-cache-cloud-grade-integrity-design.md) (CS-0054 — SHA-256 `content_hash` integrity primitive)
  - [2026-05-04-cache-backend-idempotency-design.md](./2026-05-04-cache-backend-idempotency-design.md) (CS-0055 — write-side conflict-detection contract)
  - [2026-05-04-cache-backend-streaming-design.md](./2026-05-04-cache-backend-streaming-design.md) (CS-0056 — streaming `CacheBackend` I/O shape this spec layers on top of)
**Scope:** `cli/crates/cook-fingerprint/src/backend.rs` (new `BackendConfig` struct alongside the trait), `cli/crates/cook-cache/src/backend.rs` (`LocalBackend::with_config` constructor; `max_artifact_bytes` enforcement during the streaming `put` loop; re-export of `BackendConfig`), `cli/crates/cook-cache/src/cloud_config.rs` (five optional `[cloud]` knobs and a `CloudConfig::backend_config()` accessor), and `cli/crates/cook-engine/src/run.rs` (cache-bootstrap site switched from `LocalBackend::new` to `LocalBackend::with_config(_, cloud_config.backend_config())`).

## 1. Motivation

CS-0054 / CS-0055 / CS-0056 hardened the `CacheBackend` trait's *integrity*, *write-side soundness*, and *liveness*. The surface that remains soft is *operational* — the four tunables every production cloud-cache client needs:

1. **Per-call timeout.** A hung HTTP request to R2 must not stall the build forever; the backend needs a per-call deadline.
2. **Retry-with-exponential-backoff.** Transient failures (5xx, network blips, connection resets) are routine on a shared backend; the retry policy must be parameterised.
3. **Backoff cap.** Exponential growth without a ceiling can put a single retry minutes away from the original call; a cap keeps the worst-case wait bounded.
4. **Max artifact size.** A runaway producer (a build that mistakenly emits a multi-GB log file as an output) can fill the local disk or burn cloud storage budget. The backend needs an authoritative cap that refuses oversize puts at the point of streaming, before the bytes commit.

`CloudBackend` does not exist yet — it lands in the next ticket. CS-0057's job is to lay the wiring: define the config struct, plumb it through `LocalBackend`, expose user-overrides via `.cook/cloud.toml`, and pin the `max_artifact_bytes` enforcement at the streaming-put boundary so it's testable today on the local backend. The HTTP-shaped knobs (`timeout`, `max_retries`, `backoff_initial`, `backoff_max`) are no-ops for `LocalBackend` but ride along on the same `BackendConfig` so the future `CloudBackend::with_config(_, BackendConfig)` constructor accepts the same shape with no plumbing churn.

## 2. Non-goals

- **Per-key / per-recipe overrides.** All tunables apply globally to a build's backend. A future "this recipe is allowed to emit a 10 GiB artifact" override could be a `[cache.recipes."foo"]` table, but that's a separate design.
- **Adaptive timeouts.** No automatic adjustment based on observed latency. Static caps are easier to reason about and easier to pin in tests.
- **Retry budgets across the whole build.** `max_retries` is per-call, not per-build. A build that experiences `n` calls each hitting `max_retries` worth of transients pays the full cost; a global retry budget is a separate refinement.
- **Jitter on the backoff schedule.** The schedule is deterministic exponential. Jitter is a follow-up if the deterministic schedule causes thundering-herd on the cloud side.
- **Server-side enforcement of `max_artifact_bytes`.** The cap is client-side. The cloud backend will independently enforce server-side quotas; CS-0057's `max_artifact_bytes` is a client-side fail-fast that saves the round-trip on oversized payloads.

## 3. Architecture

### 3.1 Modules touched

```
cli/crates/
├── cook-fingerprint/
│   └── backend.rs          new `BackendConfig` struct alongside the trait;
│                           `Default` impl pins the spec's default values.
├── cook-cache/
│   ├── backend.rs          `LocalBackend` gains a `config: BackendConfig`
│                           field and a `with_config(root, config)`
│                           constructor (`new(root)` becomes a thin wrapper
│                           that delegates to `with_config(root, default)`).
│                           `put` enforces `max_artifact_bytes` mid-stream:
│                           the existing 64 KiB-buffered loop accumulates
│                           total bytes; on overflow, the temp file is
│                           discarded and `BackendError::Other` returned
│                           with a message naming both the streamed total
│                           and the configured cap.
│   └── cloud_config.rs     `CloudSection` gains five optional fields
│                           (`timeout_secs`, `max_retries`,
│                           `backoff_initial_ms`, `backoff_max_ms`,
│                           `max_artifact_mib`); `CloudConfig::backend_config()`
│                           overlays them onto `BackendConfig::default()`.
└── cook-engine/
    └── run.rs              cache-bootstrap site switched from
                            `LocalBackend::new(cache_dir)` to
                            `LocalBackend::with_config(cache_dir,
                            cloud_config.backend_config())`.
```

No grammar change. No Lua API change. No on-disk schema change. No `CACHE_VERSION` bump. `CacheBackend` trait surface unchanged — `BackendConfig` is constructor-side; nothing in the trait observes it.

### 3.2 `BackendConfig` shape and defaults

```rust
#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub timeout: Duration,
    pub max_retries: u32,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub max_artifact_bytes: u64,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_retries: 3,
            backoff_initial: Duration::from_millis(100),
            backoff_max: Duration::from_secs(5),
            max_artifact_bytes: 1024 * 1024 * 1024, // 1 GiB
        }
    }
}
```

Lives in `cli/crates/cook-fingerprint/src/backend.rs` next to the trait, re-exported from `cook_cache::backend` per the existing re-export pattern. The defaults are tuned for cloud-grade workloads: 30s aligns with R2's typical end-to-end SLA, 3 retries gives ~3.5s worst-case wait under the deterministic schedule (100ms + 200ms + 400ms ≈ 700ms total backoff, plus the per-call timeout if requests time out), 5s caps the back-pressure on a flapping endpoint, and 1 GiB is large enough for typical container layers / linked binaries while small enough to fail-fast on a runaway producer.

### 3.3 `[cloud]` TOML overrides

```toml
[cloud]
enabled = true
project = "myproject"
endpoint = "https://api.cook.dev"

# CS-0057 backend tunables — all optional; absent values default.
timeout_secs       = 60
max_retries        = 5
backoff_initial_ms = 200
backoff_max_ms     = 10000
max_artifact_mib   = 512
```

Unit conventions: `_secs` for seconds, `_ms` for milliseconds, `_mib` for MiB (`* 1024 * 1024` to get bytes). Each is `Option<T>` in the deserialiser; `CloudConfig::backend_config()` overlays the present values onto `BackendConfig::default()`.

### 3.4 Why `[cloud]` and not a new `[backend]` section

The brief allowed either; placing the knobs under `[cloud]` keeps the user surface small (one place to look for cloud / backend tunables) and doesn't pre-commit to a section name a future "non-cloud streaming-S3-compatible backend" might rename. If a future config refactor splits `[cloud]` into `[cloud]` (project / endpoint / auth) and `[backend]` (tunables), a deprecation alias keeps the existing keys parsing.

## 4. Algorithms

### 4.1 `max_artifact_bytes` mid-stream enforcement

`LocalBackend::put` already streams the source through a 64 KiB buffer to a temp file, hashing as bytes flow (CS-0056 §5.2). The CS-0057 enforcement adds two lines inside the loop:

```
3. loop:
   read up to 64 KiB from `reader` into `buf` (n bytes)
   if n == 0: break
   total += n  (saturating_add to defend against u64 wraparound)
3a. NEW: if total > config.max_artifact_bytes:
        drop tmp_file; remove tmp;
        Err(BackendError::Other(format!(
            "artifact exceeds max_artifact_bytes ({total}); cap {limit}"
        )))
   hash.update(&buf[..n])
   tmp_file.write_all(&buf[..n])
4. on EOF: …
```

The check is on `total > limit`, not `total >= limit` — exactly-at-limit is permitted (the cap is "MUST NOT exceed", not "MUST be strictly less than"). The temp file is discarded before the rename-into-place commit, so no partial bytes ever surface to a reader. The diagnostic names both the streamed total (so the user knows how oversized it actually was) and the configured cap (so the user knows what value to raise if intentional).

### 4.2 Why mid-stream and not pre-flight

A pre-flight check would require the caller to declare `Content-Length` up front. The streaming `put` signature (`&mut dyn Read`) intentionally does not require that — the caller may be forwarding bytes from a streaming source whose length is not known. Mid-stream enforcement costs at most one extra 64 KiB read after the cap is breached (the loop terminates on the next iteration check), which is the same I/O the caller would have paid anyway to discover the size.

### 4.3 Why `BackendError::Other` and not a new variant

A new `BackendError::TooLarge { total, limit }` variant would be cleaner but requires a `CacheBackend` trait surface change — out-of-tree implementers must add a match arm. CS-0057 deliberately stays trait-shape-stable; the cap is a policy decision, not a new failure category. Callers that care about distinguishing oversize from generic backend errors can match on the message prefix; in practice the engine treats every `BackendError` from `put` as a non-fatal "log and continue" event, so the variant distinction is observability, not control flow.

## 5. Test plan

### 5.1 Unit tests (`cli/crates/cook-cache/src/backend.rs::tests`)

- `backend_config_default_values` — sanity-check that `BackendConfig::default()` produces (30s, 3, 100ms, 5s, 1 GiB).
- `local_backend_with_config_honored` — construct with a custom config; verify each field is observable via the `config()` accessor.
- `local_backend_put_rejects_oversize_artifact` — set `max_artifact_bytes = 100`, put 200 bytes; assert `Err`, message contains "exceeds" and "100", `BackendError::Other` variant, no artifact persisted at the key.
- `local_backend_put_accepts_artifact_at_limit` — put exactly `max_artifact_bytes` bytes; assert success and round-trip equality.

### 5.2 Unit tests (`cli/crates/cook-cache/src/cloud_config.rs::tests`)

- `backend_config_uses_defaults_when_unset` — empty `[cloud]` section produces `BackendConfig::default()` field-for-field.
- `backend_config_overrides_from_toml` — TOML with all five fields set produces the right `Duration` / `u64` values, including the `_secs` / `_ms` / `_mib` unit conversions.

### 5.3 Integration / E2E tests

No new integration tests. The existing `cache_benchmarks/verify.sh` (31 scenarios) and `cache_dep_drift/verify.sh` (3 scenarios) exercise the `LocalBackend::new` → `LocalBackend::with_config(_, default)` rewrite path; their continued green status is the regression bar.

## 6. Spec amendments

### 6.1 §{exec.cache} — one-liner permissive amendment (optional)

§8.6 already says "the hash function, on-disk layout, eviction policy, and storage medium are implementation-defined". An implementation MAY accept user-overrideable backend tunables (timeouts, retry policy, max artifact size) — this is implicit in "implementation-defined", so a normative amendment is not required. CS-0057 can append one informative sentence to §8.6 documenting that the reference implementation exposes these as `[cloud]` keys, but the Standard does not normatively require any specific knob.

Recommendation: skip the amendment. The knobs are policy, not language-surface; documenting them in the changelog (CS-0057) plus the design spec is sufficient.

## 7. Backwards compatibility

- **On-disk cache:** unchanged. `CACHE_VERSION` unchanged.
- **`CacheBackend` trait surface:** unchanged.
- **`LocalBackend::new(root)`:** preserved; delegates to `with_config(root, BackendConfig::default())`. All existing call sites (the engine bootstrap, every test, every integration fixture) continue to compile and behave identically.
- **`.cook/cloud.toml` schema:** five new optional fields under `[cloud]`, all `#[serde(default)]`. Pre-CS-0057 cloud.toml files parse identically and produce a `BackendConfig::default()`.
- **Cookfile grammar:** unchanged.
- **Cook Lua API:** unchanged.

## 8. Open questions

1. **Should `max_artifact_bytes` apply to `get`?** A pathologically-sized artifact in the backend can be `get`-ed with no cap today; the streaming reader hands the bytes to the caller, who decides what to do. Recommendation: defer. The threat (a poisoned backend serving an enormous payload) is mitigated by SHA-256 verification (the bytes either match the recorded `content_hash` or fail at EOF); a separate "max bytes consumed before EOF check" wrapper is a defence-in-depth nice-to-have, not a soundness primitive.
2. **Should `max_retries = 0` be treated as "no retries" or rejected at parse time?** Currently 0 means no retries (the first call's failure is returned directly). This is the natural reading and matches `reqwest`'s convention. No change recommended.
3. **Should the `_mib` unit eventually accept a string like `"512MiB"`?** A string-with-suffix parser is more ergonomic but adds parse-error surface. Deferred until the user-feedback signal exists; the integer-MiB form is unambiguous.
4. **Should `LocalBackend` log a `tracing::debug!` line at construction when a non-default config is honoured?** Useful for "did my cloud.toml actually take effect" diagnostics. Recommendation: add when the cloud backend lands; the local-only path today has no observability around backend identity beyond "did it work".

## Appendix A. Why the `_secs` / `_ms` / `_mib` unit suffix

TOML's primitive types are `i64`, `f64`, `bool`, `string`, datetime, and arrays/tables. There is no native `Duration` type. The two ergonomic options are:

1. **Unit suffix in the key name** (chosen). `timeout_secs = 30` is unambiguous; the key tells the reader the unit. The deserialiser stays a plain `Option<u64>`.
2. **String parser.** `timeout = "30s"` reads naturally but requires a custom deserialiser that handles `"500ms"`, `"30s"`, `"1m"`, etc., and surfaces parse errors at TOML-load time rather than constructor time.

The suffix-in-key form composes cleanly with `serde`'s default machinery (no custom impl), is self-documenting (the user sees `timeout_secs` and knows seconds without reading docs), and matches the convention in similar Rust ecosystem tools (e.g. `rustls-config`'s `timeout_ms`). A future refactor to string-parsed durations is straightforward — accept both, prefer string when both are set, deprecate the suffix form on a major version bump.

## Appendix B. Why `Duration` and not raw seconds in the struct

`BackendConfig` is the in-memory shape consumed by backend code. Backend code uses `std::time::Duration` for `reqwest::Client::builder().timeout(_)`, `std::thread::sleep(_)`, etc. Converting from `u64` seconds to `Duration` at every consumption site is churn; converting once at the cloud-config-to-BackendConfig boundary is the natural seam. The TOML side uses `u64` because that's what TOML serialises cleanly; the in-memory side uses `Duration` because that's what consumers want. The conversion happens in `CloudConfig::backend_config()` — one place, one direction, no ambiguity.
