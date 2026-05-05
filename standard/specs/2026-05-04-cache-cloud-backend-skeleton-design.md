# Design: `CloudBackend` skeleton — sync HTTP client over a v1 wire protocol

**Date:** 2026-05-04
**Status:** Design — pending implementation plan
**Standard change ID:** CS-0058
**Linear epic:** SHI-24 cache cloud-readiness — *the actual HTTP client*
**Predecessors:**
  - [2026-05-01-cache-cloud-readiness-design.md](./2026-05-01-cache-cloud-readiness-design.md) (introduced the `CacheBackend` trait, `cloud_key`, the `LocalBackend`, and the seam this client plugs into)
  - [2026-05-04-cache-cloud-grade-integrity-design.md](./2026-05-04-cache-cloud-grade-integrity-design.md) (CS-0054 — SHA-256 `content_hash` integrity primitive carried over the wire as `X-Cook-Content-Hash`)
  - [2026-05-04-cache-backend-idempotency-design.md](./2026-05-04-cache-backend-idempotency-design.md) (CS-0055 — write-side conflict-detection contract surfaced over HTTP as `409 Conflict`)
  - [2026-05-04-cache-backend-streaming-design.md](./2026-05-04-cache-backend-streaming-design.md) (CS-0056 — streaming I/O shape that maps cleanly onto HTTP request/response bodies)
  - [2026-05-04-cache-backend-config-design.md](./2026-05-04-cache-backend-config-design.md) (CS-0057 — `BackendConfig` whose four HTTP-shaped knobs `CloudBackend` is the first consumer of)
**Scope:** new `cli/crates/cook-cache/src/cloud_backend.rs` (`CloudBackend` struct implementing `CacheBackend` over HTTP/1.1; helpers for retry-with-jittered-exponential-backoff, header packing/parsing, status-code-to-`BackendError` mapping); `cli/crates/cook-cache/src/cloud_config.rs` (new optional `api_key: Option<String>` field on `CloudSection`; new `CloudConfigError::MissingApiKey` variant; new `resolved_api_key()` accessor that prefers `COOK_CLOUD_API_KEY` over the TOML field; `load_or_default` validation extended to require the resolved API key when `cloud.enabled = true`); `cli/crates/cook-cache/src/lib.rs` (one new `pub mod cloud_backend;` line plus a `pub use` re-export); `cli/crates/cook-cache/Cargo.toml` (three new runtime dependencies — `ureq`, `serde_json` (already present, used via batch-query body), `base64` (deferred — see §3.5); one new dev-dependency — `mockito`); `cli/crates/cook-engine/src/run.rs` (cache bootstrap branches on `cloud_config.cloud.enabled` to construct `CloudBackend` or `LocalBackend`, with the same `cloud_config.backend_config()` shape and the same health-check pattern in either branch).

## 1. Motivation

CS-0054 / CS-0055 / CS-0056 / CS-0057 hardened the `CacheBackend` trait — its integrity, write-side soundness, liveness, and operational tunables. The trait is now ready to be implemented over a network. CS-0058 is that implementation: the *client side* of the wire protocol that the future Cook Cloud (SaaS) artifact server will speak.

Cook Cloud does not exist yet. The job here is to lock down the protocol shape with a working client implementation, prove it against a mock HTTP server, and ship it behind `cloud.enabled = true` so the local-only path is unaffected. Once the server lands, the only delta on the client side will be wiring real credentials and pointing at the production endpoint.

The protocol shape must:

1. **Stream bodies in both directions** — multi-GB artifacts cannot be buffered on either side (CS-0056).
2. **Carry the integrity primitive over the wire** — `X-Cook-Content-Hash` matches what `LocalBackend` writes to the sidecar; the client wraps the response body in the same `VerifyingReader` it uses on disk (CS-0054).
3. **Surface conflicts as a distinct status code** — `409 Conflict` maps to `BackendError::Other("conflict at <key>: ...")` so the caller distinguishes "different bytes already at this key" from generic transport errors (CS-0055).
4. **Honour every `BackendConfig` knob** — per-call timeout, max-retries with exponential-and-jittered backoff, max-artifact-bytes streaming check (CS-0057).
5. **Stay sync** — the `CacheBackend` trait is sync; introducing tokio for one backend would force the whole engine onto an async runtime. `ureq` is the natural sync choice (rustls TLS, ~10× lighter than `reqwest`).
6. **Fail closed on auth.** A missing API key when `cloud.enabled = true` is a build-start error, identical in shape to CS-0001's `MissingProject`.

## 2. Non-goals

- **Implementing the server side.** Cook Cloud's artifact server is future work in a separate ticket. CS-0058 implements only the HTTP client; the server is a black-box mocked via `mockito` in unit tests.
- **Multipart / chunked-resumable uploads.** v1 is single-request streaming PUT. A future "resumable upload" extension would land as a separate spec layered on top.
- **Connection pooling tuning.** `ureq::Agent` defaults are accepted; per-host pool size, idle timeout, etc. are deferred.
- **Server-side `max_artifact_bytes` enforcement.** The cap is client-side per CS-0057 §2; the server independently enforces a server-side quota and signals violation via `413 Payload Too Large`. CS-0058 maps `413` to `BackendError::Other` so the engine logs and continues.
- **Retry budgets across the whole build.** `max_retries` is per-call, identical to CS-0057's local-backend semantics.
- **Authentication beyond a single bearer token.** OAuth, mTLS, signed-URL auth, etc. are deferred. Bearer-token-from-env-or-TOML covers the SaaS launch shape.
- **Cache invalidation / eviction signalling.** The server may evict; the client treats eviction as `404 Not Found` and surfaces a miss. No client-side eviction protocol.
- **Compression.** v1 ships uncompressed bodies. The Standard's bytes-of-the-artifact contract is untouched; gzip/zstd content-encoding is a transparent transport optimisation that can land later without protocol bump.

## 3. Architecture

### 3.1 Modules touched

```
cli/crates/
├── cook-cache/
│   ├── Cargo.toml          new runtime dependencies: `ureq` (sync HTTP +
│                           rustls TLS), `serde_json` already present and
│                           used for batch-query bodies; new dev-dependency:
│                           `mockito` for HTTP mocking.
│   ├── src/
│   │   ├── lib.rs          `pub mod cloud_backend;` plus re-export of
│   │                       `CloudBackend`.
│   │   ├── cloud_config.rs new optional `api_key: Option<String>` on
│   │                       `CloudSection`; new `MissingApiKey` error
│   │                       variant; `resolved_api_key()` accessor that
│   │                       prefers `COOK_CLOUD_API_KEY` env var over the
│   │                       TOML field; `load_or_default` validation
│   │                       requires both `endpoint` and a resolved API
│   │                       key when `cloud.enabled = true`.
│   │   └── cloud_backend.rs    NEW. `pub struct CloudBackend` with
│   │                           `endpoint`, `api_key`, `client: ureq::Agent`,
│   │                           `config: BackendConfig`. `impl CacheBackend`
│   │                           with all five trait methods: `batch_query`,
│   │                           `get` (returns `Box<VerifyingReader<...>>`),
│   │                           `put` (streams + enforces
│   │                           `max_artifact_bytes`), `delete`, `health`.
│   │                           Internal helpers: retry-with-jittered-
│   │                           exponential-backoff, header packing /
│   │                           parsing, status-code-to-`BackendError`
│   │                           mapping.
└── cook-engine/
    └── src/run.rs          cache bootstrap branches on
                            `cloud_config.cloud.enabled`; constructs
                            `CloudBackend` from the resolved API key +
                            endpoint + `cloud_config.backend_config()` on
                            the cloud branch, falls through to the
                            existing `LocalBackend::with_config` path
                            otherwise. Health check applies in either
                            branch.
```

No grammar change. No Lua API change. No on-disk schema change. No `CACHE_VERSION` bump. The `CacheBackend` trait surface is unchanged — `CloudBackend` is a new implementer of an existing trait.

### 3.2 Wire protocol — endpoints, methods, headers, status codes

All paths are versioned under `/v1/`. The base URL is `cloud_config.cloud.endpoint` (e.g. `https://api.cook.dev`); the client appends `/v1/...` to that base for every request.

#### 3.2.1 Auth — applied to every request

```
Authorization: Bearer <api_key>
```

The API key is sourced at backend construction time via `CloudConfig::resolved_api_key()`:

1. **`COOK_CLOUD_API_KEY` env var** — first priority. Wins over the TOML field if both are set; the rationale is identical to twelve-factor: secrets belong outside checked-in config.
2. **`[cloud] api_key = "..."` in `.cook/cloud.toml`** — second priority. Convenient for local development where the user has already opted into `[cloud]` config; not recommended for shared / CI-checked-in repositories.

When `cloud.enabled = true`:
- `endpoint` MUST be set.
- A resolved API key MUST be obtainable (env or TOML).

A missing API key surfaces as `CloudConfigError::MissingApiKey` from `load_or_default`, identical in shape to the existing `MissingProject` build-start error. No HTTP request is ever sent without a bearer token attached.

#### 3.2.2 Key encoding in URLs

The 32-byte `CloudKey` is hex-encoded (lowercase, 64 chars) and embedded as a path segment. Hex is chosen over base64url because URL routers across the stack (R2, Cloudflare, nginx, Caddy) handle hex paths uniformly and because it matches the on-disk encoding `LocalBackend::path_for` already uses — symmetry across the two backends keeps debugging diagnostics easy to cross-reference.

#### 3.2.3 `POST /v1/artifacts/batch_query` — existence check

**Request body** (JSON):
```json
{ "keys": ["aabbccdd...64hex...", "112233...", ...] }
```

**Response 200** (JSON):
```json
{ "present": ["aabbccdd...64hex...", ...] }
```

The `present` array is the subset of `keys` that exist on the server. Order is not significant; `CloudBackend::batch_query` collects into a `BTreeSet<CloudKey>` (matching the trait's return type).

#### 3.2.4 `GET /v1/artifacts/{key_hex}` — fetch artifact bytes

**Response 200**: bytes streamed in body. Headers carry the `ArtifactMeta`:

| Header                  | Type     | Required? | Maps to `ArtifactMeta` field |
| ----------------------- | -------- | --------- | ---------------------------- |
| `X-Cook-Content-Hash`   | hex(32B) | YES       | `content_hash` (load-bearing for `VerifyingReader`) |
| `X-Cook-Size-Bytes`     | u64      | YES       | `size_bytes` |
| `X-Cook-Schema-Version` | u32      | YES       | `schema_version` |
| `X-Cook-Recipe-Namespace` | string | YES       | `recipe_namespace` |
| `X-Cook-Output-Index`   | u32      | YES       | `output_index` |
| `X-Cook-Output-Path`    | string   | YES       | `output_path` |

The optional `ArtifactMeta` fields (`command_hash`, `context_hash`, `env_contribution`, `tags`, `consulted_env_keys`) are **deliberately skipped in v1 headers**. The engine's restore path consumes only the bytes (the bytes are what installs to the working directory) and the `content_hash` (for verification); the rest of the meta is diagnostic-only on the local backend (it's logged when present, not consulted for control flow). Carrying every field over the wire as a header would balloon header-size for marginal value; v1 stays minimal. A future "rich meta" extension can land as a separate header (`X-Cook-Meta-Json: <base64-json-blob>`) without protocol bump.

The response body is wrapped in a `VerifyingReader<R>` keyed on the `X-Cook-Content-Hash` value, exactly as `LocalBackend::get` wraps a `File`. Same fail-closed semantics: bytes-then-finalize-at-EOF, mismatch raises `io::Error` of `InvalidData` from the EOF read.

**Response 404**: not present. Returned to the caller as `Ok(None)`. NOT an error.

**Response 401**: auth failure. Maps to `BackendError::Unauthorized(<diagnostic>)`.

**Response 429**: rate-limited. Maps to `BackendError::QuotaExceeded`.

**Response 5xx**: transient. Maps to `BackendError::Transient(<diagnostic>)`. Engine treats this as miss-and-continue.

#### 3.2.5 `PUT /v1/artifacts/{key_hex}` — upload artifact bytes

**Request body**: bytes streamed from the caller's reader. The same six required `X-Cook-*` headers are sent by the client.

`X-Cook-Content-Hash` MAY be the zero sentinel (`0000...0000` × 32 bytes hex) when the caller has not pre-computed the hash — the server SHOULD stamp the computed hash on receipt and persist it. When non-zero, the server SHOULD verify by hashing the received bytes and reject with `400 Bad Request` (mapped to `BackendError::Other("bad request: ...")`) on mismatch. This matches `LocalBackend::put`'s caller-claimed-hash policy.

The client enforces `max_artifact_bytes` mid-stream on its own reader before bytes ever reach the wire — same loop shape as `LocalBackend::put`'s `total > limit` check. On overflow, the request is aborted (the in-flight body write is dropped) and `BackendError::Other("artifact exceeds max_artifact_bytes ({total}); cap {limit}")` is returned. This saves the round-trip cost on oversize payloads; the server independently enforces its own quota via `413`.

**Response 200 / 201**: success.

**Response 409 Conflict**: CS-0055 — different bytes already at this key. Maps to `BackendError::Other("conflict at <key>: server-side bytes differ")`. The diagnostic names the hex key.

**Response 413 Payload Too Large**: server-side `max_artifact_bytes` exceeded. Maps to `BackendError::Other("server rejected oversize artifact: <diagnostic>")`. Distinct from the client-side check (which never sends the bytes); this only fires when the server's cap is tighter than the client's.

**Response 401 / 429 / 5xx**: as in §3.2.4.

#### 3.2.6 `DELETE /v1/artifacts/{key_hex}` — explicit deletion

**Response 204**: success.

**Response 404**: idempotent success — the trait contract says `delete` of a missing key is `Ok(())`, so the client folds 404 into success.

**Response 401**: maps to `BackendError::Unauthorized`.

#### 3.2.7 `GET /v1/health` — liveness probe

**Response 200**: healthy. Body is ignored.

**Anything else**: maps to `BackendError::Transient`. The engine's existing pattern at `cook-engine::run::run_inner` logs "cache backend unavailable" once and continues with the backend disabled for the build — identical behaviour to a `LocalBackend` whose root is unreadable.

### 3.3 Status-code-to-`BackendError` mapping (consolidated)

| HTTP status | `BackendError` variant | Retried? | Engine reaction |
| ----------- | ---------------------- | -------- | --------------- |
| 200 / 201 / 204 | (success — not an error) | n/a | normal path |
| 400         | `Other("bad request: ...")` | NO | log; treat as miss / drop put |
| 401         | `Unauthorized(...)`     | NO       | log once; disable backend for build |
| 404 (`get`) | (not an error — `Ok(None)`) | n/a | miss |
| 404 (`delete`) | (not an error — `Ok(())`) | n/a | idempotent success |
| 409         | `Other("conflict at <key>: ...")` | NO | CS-0055 — log; drop put |
| 413         | `Other("server rejected oversize artifact: ...")` | NO | log; drop put |
| 429         | `QuotaExceeded`         | NO       | log; drop put / treat as miss |
| 5xx         | `Transient(...)`        | YES      | retry per CS-0057 backoff |
| network err | `Transient(...)`        | YES      | retry per CS-0057 backoff |

The "retried?" column is the single source of truth for §4.1's retry policy.

### 3.4 Auth — no API key in diagnostics, ever

Every diagnostic surfaced to the user via `BackendError::Unauthorized` includes the *response body's diagnostic* (server-supplied) but NEVER the request's `Authorization` header value. The `tracing` debug logs at the request boundary likewise redact `Authorization: Bearer <REDACTED>`. This is a discipline the client implementation MUST honour at every request-construction site; cf. §6.2 test plan for the regression check.

### 3.5 Why `base64` is deferred (and why the brief mentioned it)

The brief suggested adding `base64` for "hex-encoding `content_hash` in headers" — this was an editorial slip; hex encoding does not need `base64`. The `hex` crate is already a runtime dependency of `cook-cache` and handles both directions of the `content_hash` round-trip. CS-0058 ships without `base64`. If a future "rich meta header" lands (`X-Cook-Meta-Json: <base64-json>`) it can pull `base64` in then.

## 4. Algorithms

### 4.1 Retry-with-jittered-exponential-backoff

Honoured by every HTTP-emitting method (`batch_query`, `get`, `put`, `delete`, `health`) in `CloudBackend`. The retry shell is a helper:

```text
retry_loop(config, op):
    let mut attempt = 0
    let mut delay = config.backoff_initial
    loop:
        match op() of
            Ok(v) => return Ok(v)
            Err(BackendError::Transient(_)) if attempt < config.max_retries =>
                jittered = delay ± 25%   (uniform random in [0.75*delay, 1.25*delay])
                jittered = min(jittered, config.backoff_max)
                sleep(jittered)
                delay = min(delay * 2, config.backoff_max)
                attempt += 1
                continue
            Err(other) => return Err(other)
```

- **Only `Transient` retries.** `Unauthorized`, `QuotaExceeded`, `Other` (including `409` conflict and `413` oversize) are returned immediately. `400 Bad Request` is also non-retryable. This is the column "Retried? — NO" in §3.3.
- **Jitter is ±25% uniform.** Without jitter, a thundering herd of clients all hitting a flapping endpoint synchronise their retries. ±25% breaks the synchronisation cheaply. The CS-0057 spec called jitter "a follow-up if the deterministic schedule causes thundering-herd"; CS-0058 lands the jitter on the cloud client because the cost is one `rand::random::<f64>()` per retry and the soundness benefit is non-trivial in a multi-tenant setting.
- **Cap on `delay`.** Each backoff step doubles, but is clamped at `config.backoff_max`. Default `backoff_max = 5s`, so the schedule under defaults (`max_retries = 3`, `backoff_initial = 100ms`) is roughly `[100ms, 200ms, 400ms]` with ±25% — total worst-case wait under retries is ~875ms plus the per-call timeouts.
- **Total time budget.** Per CS-0057 §1: total time per call should not exceed roughly `timeout + sum(backoffs) + per_call_timeout * (max_retries + 1)`. CS-0058 does not enforce a wall-clock budget — the per-call `timeout` plus `max_retries` is the only knob; if a user wants tighter, they can lower either. The wall-clock budget is documented in the `BackendConfig` doc-comment for tuning.

### 4.2 `max_artifact_bytes` mid-stream enforcement on `put`

Same shape as `LocalBackend::put`'s loop (CS-0057 §4.1). The client wraps the caller's `reader` in a counting wrapper that:

1. Reads up to 64 KiB at a time from the source.
2. Accumulates `total` via `saturating_add`.
3. On `total > config.max_artifact_bytes`, returns an `io::Error` of `Other` kind (which `ureq` surfaces as a transport failure on the request's `send(reader)` call), and the client maps that surface back to `BackendError::Other("artifact exceeds max_artifact_bytes ({total}); cap {limit}")`.
4. Aborts the in-flight HTTP request — the partial body is closed, `ureq` cleans up the connection.

The server never sees a complete oversize payload through this client. A misbehaving client (one that bypasses the wrapper) is independently caught by the server's `413` cap (§3.3).

### 4.3 `get` — wrapping the response body in `VerifyingReader`

The `LocalBackend::get` flow reads the sidecar's `content_hash`, opens the file, wraps in `VerifyingReader`. The cloud flow is structurally identical:

1. Issue `GET /v1/artifacts/{key_hex}` under the retry shell.
2. On `200`, parse `X-Cook-Content-Hash` (must be present, must be 64-char lowercase hex). Missing or malformed → `BackendError::Other("malformed response: missing X-Cook-Content-Hash")` (NOT `Ok(None)`; this is server misbehaviour, not a miss).
3. Take the response's body reader (`response.into_reader()`).
4. Wrap in `VerifyingReader::new(body, expected_hash)`.
5. Return `Ok(Some(Box::new(verifier)))`.

EOF-time mismatch surfaces as the same `io::Error(InvalidData)` as the local path. Callers using `get_bytes` get the same `Ok(None)` fold; callers using the streaming `get` see the error at `read_to_end`. Crucially: this means a multi-GB response is never buffered in memory — the verification streams identically to the local backend's `File` case.

### 4.4 Header packing on `put`

```text
X-Cook-Content-Hash:  hex(meta.content_hash)        (zero sentinel allowed)
X-Cook-Size-Bytes:    meta.size_bytes               (decimal)
X-Cook-Schema-Version: meta.schema_version          (decimal)
X-Cook-Recipe-Namespace: meta.recipe_namespace      (string, US-ASCII; non-ASCII percent-encoded if needed)
X-Cook-Output-Index:  meta.output_index             (decimal)
X-Cook-Output-Path:   meta.output_path              (string)
```

The client does NOT consult or modify `meta.content_hash` after the `put` returns — the server is the authority on the persisted hash. Unlike `LocalBackend::put` (which stamps the computed hash into `meta` in-place), `CloudBackend::put` cannot stamp without hashing client-side, which would force buffering or duplicate-pass-over-bytes. The trade-off: cloud callers who pass the zero sentinel observe the zero sentinel in `meta.content_hash` after `put` returns. This is an intentional asymmetry; a future enhancement could have the server return the computed hash in a `200 OK` response header (`X-Cook-Server-Stamped-Content-Hash`) which the client copies into `meta.content_hash`.

### 4.5 `batch_query` — `serde_json` body

```rust
#[derive(Serialize)]
struct BatchQueryRequest { keys: Vec<String> }   // hex-encoded

#[derive(Deserialize)]
struct BatchQueryResponse { present: Vec<String> }
```

The implementation hex-encodes the input `&[CloudKey]` once into the request body, decodes the `present` array back into `CloudKey` (errors if any string is not 64 hex chars), and collects into a `BTreeSet<CloudKey>` matching the trait return.

## 5. Auth model — the `api_key` field

### 5.1 `CloudSection::api_key`

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CloudSection {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub endpoint: Option<String>,
    #[serde(default)] pub project: Option<String>,
    #[serde(default)] pub api_key: Option<String>,    // NEW (CS-0058)
    // ... CS-0057 backend tunables unchanged ...
}
```

### 5.2 `CloudConfig::resolved_api_key`

```rust
pub fn resolved_api_key(&self) -> Option<String> {
    if let Ok(v) = std::env::var("COOK_CLOUD_API_KEY") {
        if !v.is_empty() { return Some(v); }
    }
    self.cloud.api_key.clone()
}
```

Env var wins over TOML. An empty env var (`COOK_CLOUD_API_KEY=""`) falls through to the TOML field — this matches the spirit of "an env var that's set to empty is the same as unset", which is the convention in twelve-factor tooling.

### 5.3 `load_or_default` validation extended

```text
if cfg.cloud.enabled:
    if cfg.cloud.project.is_none(): err MissingProject       (existing)
    if cfg.cloud.endpoint.is_none(): err MissingEndpoint     (NEW)
    if cfg.resolved_api_key().is_none(): err MissingApiKey   (NEW)
```

`MissingEndpoint` is technically a tighter constraint than CS-0057's "endpoint is optional"; the rationale is that endpoint-without-cloud-enabled is allowed, but cloud-enabled-without-endpoint is a config bug — the engine can't construct a `CloudBackend` without one. This is the CS-0058 increment to validation.

### 5.4 Diagnostic shape for `MissingApiKey`

```
[cloud] enabled = true but no API key resolved —
set `api_key = "..."` in .cook/cloud.toml or
export COOK_CLOUD_API_KEY=<your-token>
```

Identical structure to the existing `MissingProject` message.

## 6. Test plan

### 6.1 Unit tests (`cli/crates/cook-cache/src/cloud_backend.rs::tests`) — `mockito`-based

- `cloud_backend_get_round_trips` — mock GET returning bytes plus `X-Cook-Content-Hash` matching `SHA-256(bytes)`; client `get`, `read_to_end`, asserts bytes match.
- `cloud_backend_get_returns_none_on_404` — mock 404, `get` returns `Ok(None)`.
- `cloud_backend_get_fails_closed_on_byte_tamper` — mock GET returns bytes whose SHA-256 differs from `X-Cook-Content-Hash`; `read_to_end` on the verifier raises `InvalidData`.
- `cloud_backend_put_round_trips` — mock PUT, body received matches input, `Ok(())` returned.
- `cloud_backend_put_rejects_oversize` — `BackendConfig::max_artifact_bytes = 100`, push 200 bytes through `put`, returns `Err` with "exceeds" and "100" in the message; mock not called (or called with a partial body before abort — implementation detail).
- `cloud_backend_put_handles_409_conflict` — mock 409, `put` returns `Err(BackendError::Other(_))` whose message contains "conflict".
- `cloud_backend_batch_query_round_trips` — mock POST returning JSON `{"present":[...]}`, client returns the right `BTreeSet<CloudKey>`.
- `cloud_backend_retries_on_5xx` — mock first call 503, second call 200; assert exactly 2 calls fired (mockito `expect(2)`).
- `cloud_backend_does_not_retry_on_401` — mock 401; assert exactly 1 call fired (mockito `expect(1)`).
- `cloud_backend_health_returns_ok_on_200` — mock GET `/v1/health` 200, `health()` returns `Ok(())`.
- `cloud_backend_health_returns_transient_on_5xx` — mock GET `/v1/health` 503 (after retries are exhausted); `health()` returns `BackendError::Transient`.
- `cloud_backend_unauthorized_maps_correctly` — covered by the 401 test above.

### 6.2 Unit tests (`cli/crates/cook-cache/src/cloud_config.rs::tests`)

- `cloud_enabled_requires_api_key` — `enabled = true` plus `endpoint` plus `project` but no `api_key` and no env var → `MissingApiKey`.
- `cloud_enabled_uses_env_var_api_key` — set `COOK_CLOUD_API_KEY`, no TOML field, validation passes.
- `cloud_enabled_uses_toml_api_key` — TOML `api_key = "..."`, no env var, validation passes.

(The env-var test must serialise across other env-var-touching tests in the same crate; Rust's parallel test runner can otherwise race the global env. Use a `Mutex<()>` guard or `set_var`/`remove_var` in a controlled order.)

### 6.3 Integration / E2E tests

No new integration tests in this CS. The existing `cache_benchmarks/verify.sh` (31 scenarios) and `cache_dep_drift/verify.sh` (3 scenarios) exercise the `LocalBackend` path; their continued green status under the engine's bootstrap branching (cloud disabled → local backend) is the regression bar. End-to-end coverage of the cloud path requires a real server and lands when Cook Cloud ships.

## 7. Spec amendments

None normative. The wire protocol is implementation-defined per §{exec.cache} ("the on-disk layout, eviction policy, and storage medium are implementation-defined" — the network protocol is the cloud backend's "storage medium"). CS-0058 is a reference-implementation amendment.

If a future tightening is desired — e.g. a normative "an artifact backend MUST verify the `content_hash` of bytes it returns" — that would amend §{exec.cache.integrity} in a separate ticket.

## 8. Backwards compatibility

- **On-disk cache:** unchanged. `CACHE_VERSION` unchanged.
- **`CacheBackend` trait surface:** unchanged. `CloudBackend` is a new implementer.
- **`.cook/cloud.toml` schema:** one new optional field (`api_key`) under `[cloud]`. Pre-CS-0058 cloud.toml files parse identically. Validation tightens *only* when `cloud.enabled = true` — for the disabled path (the default), no change.
- **`LocalBackend::with_config`:** unchanged. The bootstrap site in `run.rs` adds a branch on `cloud.enabled`, but the local branch is byte-for-byte the existing call.
- **Cookfile grammar:** unchanged.
- **Cook Lua API:** unchanged.

## 9. Open questions

1. **Should the client parse a `Retry-After` header on 429?** Standard convention says yes. v1 ignores it (the `BackendError::QuotaExceeded` path drops the put / treats the get as miss; no retry happens). When the Cloud SaaS lands with rate limits, honouring `Retry-After` for *queued* puts (a future "background put" path) is reasonable. v1 keeps the synchronous "drop and continue" shape.
2. **Should `health()` retry?** It does today (the retry shell wraps it). For a build-start liveness check, retrying a 5xx response 3 times costs ~1s of wall-clock under defaults. Recommendation: keep retrying; if it slows build-start enough to matter, gate `health()` on a separate `health_max_retries: u32 = 0` knob in a future spec.
3. **Should the client honour HTTP/2?** `ureq` 2.x is HTTP/1.1 only. HTTP/2 multiplexing is a meaningful win for `batch_query` parallelism; for v1 the protocol is `batch_query`-already-batched, so single-connection HTTP/1.1 is fine. If `batch_query` ever fanouts into N concurrent calls, revisit.
4. **Should `X-Cook-Content-Hash` ever travel as `Content-Digest` (RFC 9530)?** RFC 9530 is the standard HTTP digest header. v1 ships with `X-Cook-Content-Hash` because it's project-specific and unambiguous; aligning to RFC 9530 in a v2 of the wire protocol is straightforward.
5. **Should `put` ever retry on 5xx?** It does today (5xx → `Transient` → retry). This is mostly safe — the server is the source of truth on idempotency (CS-0055), so a retry of a `put` that succeeded server-side but timed-out client-side will get a `200` (re-put of identical bytes is idempotent at the server). A retry that races against a different client's conflicting write would surface as 409 on the retry, which is the correct shape. If the server is the kind that returns 5xx for "I succeeded but my response didn't reach you", the retry may surface a 409-on-self that's actually self-conflict; handling that is server-design territory. v1 retries.
6. **`api_key` in committed `cloud.toml` — security concern.** The `[cloud] api_key = "..."` form is convenient but can leak via `git add .cook/`. CS-0058 documents in the user-facing changelog that the env-var form is recommended for shared / CI repositories; a follow-up could emit a `tracing::warn!` at build start if `api_key` is set in TOML and the file is checked into git. Out of scope for v1.

## Appendix A. Why `ureq`, not `reqwest`

| Dimension          | `ureq` 2.x                          | `reqwest` 0.12               |
| ------------------ | ----------------------------------- | ---------------------------- |
| Sync API           | Native (the only API)               | Optional (`blocking` feature) |
| TLS                | rustls (`tls` feature)              | rustls or native-tls          |
| Tokio dependency   | NONE                                | YES (in async; `blocking` still pulls in tokio for the runtime) |
| Compile time       | Light (~2s clean)                   | Heavy (~25s clean)            |
| Code size          | ~150 KB compiled                    | ~2 MB compiled                |
| HTTP/2             | NO                                  | YES                          |
| Streaming bodies   | YES (`request.send(reader)` / `response.into_reader()`) | YES |

The `CacheBackend` trait is sync (CS-0056 §3 — sync trait so the engine doesn't have to host an async runtime). `reqwest::blocking` would pull tokio in transitively for one backend, ballooning compile time and binary size. `ureq` is the natural fit. HTTP/2 isn't a v1 requirement (§9 Q3).

## Appendix B. Why the `X-Cook-` header prefix

Every project-specific header is prefixed `X-Cook-` (six letters, terminator). RFC 6648 deprecated the `X-` prefix convention in 2012 in favour of "use a normal header name", but every working IANA-registered alternative requires *registering* the header — out of scope for an internal SaaS client. The `X-Cook-` prefix is namespaced enough that no infrastructure intermediary will collide on it; it's a defensible compromise.

If a future open-protocol spec emerges (e.g. Cook joins a cross-vendor "build artifact protocol" effort), the header names migrate to the registered form in a v2 protocol bump.

## Appendix C. Why hex, not base64url

Hex doubles the on-the-wire size relative to base64url (64 chars vs 44). For URL paths this is negligible (the URL is already ~20 chars of base + path); for headers (`X-Cook-Content-Hash`) it's a marginal cost. The benefits:

1. **Symmetry with on-disk encoding.** `LocalBackend::path_for` uses hex; cross-referencing a server-side log entry against a local cache directory is grep-able.
2. **Universally supported.** Every router, proxy, log aggregator handles `[0-9a-f]+` paths; base64url's `+/=` characters can need escaping in some configurations.
3. **Lowercase canonicalisation.** Lowercase hex is the de-facto convention for SHA-256; the implementation pins `hex::encode` (which produces lowercase) and rejects mixed-case server responses with `BackendError::Other("malformed hex: ...")`.

Base64url would be reasonable. Hex is what `LocalBackend` already uses; the cost of switching is more than the cost of staying.
