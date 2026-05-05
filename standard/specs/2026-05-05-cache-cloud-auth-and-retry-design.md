# Design: Cloud-auth simplification + `Retry-After` honour for `CloudBackend`

**Date:** 2026-05-05
**Status:** Design — pending implementation plan
**Standard change ID:** CS-0059
**Linear epic:** SHI-24 cache cloud-readiness — *BetterAuth + Cloudflare Workers fit-up*
**Predecessors:**
  - [2026-05-04-cache-cloud-backend-skeleton-design.md](./2026-05-04-cache-cloud-backend-skeleton-design.md) (CS-0058 — the wire protocol this spec adjusts)
  - [2026-05-04-cache-backend-config-design.md](./2026-05-04-cache-backend-config-design.md) (CS-0057 — `BackendConfig` whose `backoff_initial` / `backoff_max` knobs CS-0059's clamp uses)
**Scope:** `cli/crates/cook-cache/src/cloud_config.rs` (drop `api_key: Option<String>` from `CloudSection`; rewrite `resolved_api_key` as env-var-only; tighten `MissingApiKey` diagnostic), `cli/crates/cook-fingerprint/src/backend.rs` (`BackendError::QuotaExceeded` becomes `QuotaExceeded(Option<Duration>)`), `cli/crates/cook-cache/src/cloud_backend.rs` (new `parse_retry_after` helper, refactored `map_status_error` / `map_ureq_error` thread the parsed hint, `retry_loop` gains a quota-retry arm bounded by `[backoff_initial, backoff_max]`).

## 1. Motivation

The user is about to build the Cook Cloud server on **BetterAuth** (auth) + **Cloudflare Workers** (runtime, R2 for artifact storage). A close compatibility read of the CS-0058 wire protocol against that stack found it lands cleanly — Bearer-token auth, plain HTTP shapes, streaming bodies, custom `X-Cook-*` headers, and the standard status-code set all map directly onto Workers + BetterAuth.

Two small client adjustments fall out of the BetterAuth + CF context, both worth doing now under one CS-0059 commit before the server work begins:

1. **Drop the `[cloud] api_key` TOML field.** It was the CS-0058 §9 Q6 foot-gun. With BetterAuth long-lived prefix-tagged keys this becomes the kind of secret you really do not want git-committed. Local devs will get an interactive `cook cloud login` flow in a future spec; CI runners use `COOK_CLOUD_API_KEY` as a secret-store env var (the standard pattern). v1 ships env-var-only — clean and explicit.
2. **Honour `Retry-After` on 429.** CS-0058 §9 Q1 punted this to a follow-up. Cloudflare's Rate Limiter binding emits `Retry-After: <delta-seconds>` natively, and BetterAuth's rate-limit middleware can too — implementing it now means smooth backoff on day one of the server build.

The bigger interactive-auth flow (`cook cloud login`, OS keyring, GitHub Actions OIDC trust) is intentionally **out of scope** for CS-0059. It is a multi-week design that warrants its own spec; CS-0059 narrows the surface so that future spec lands without a config-shape conflict.

## 2. Non-goals

- **Interactive `cook cloud login` flow.** Subcommand structure, OS-keyring integration (`keyring` crate, macOS Keychain / Windows Credential Manager / libsecret), device-code OAuth, and credential-precedence rules ship in a later spec. CS-0059's resolution path stays env-var-only.
- **GitHub Actions OIDC trust path.** Exchanging an Actions OIDC token for a short-lived API key (sigstore / Vercel / GCP / AWS pattern) is the right CI story long-term but is separate work. v1 CI uses `COOK_CLOUD_API_KEY` as an Actions secret.
- **HTTP-date form of `Retry-After`.** RFC 9110 §10.2.3 permits both delta-seconds and HTTP-date forms. v1 supports delta-seconds only; HTTP-date falls through to `None` (terminal). CF's Rate Limiter and BetterAuth's rate-limit middleware emit delta-seconds, so this is the form we'll see in practice. HTTP-date support is a v2 polish.
- **Multi-bucket `Retry-After` semantics.** A server that emits a single hint applies that hint to the next request only; CS-0059 does not implement per-endpoint or per-tenant clamps. The `BackendConfig::backoff_max` clamp is the only safety net.
- **Standard normative-chapter amendment.** Same rationale as CS-0058 §6: the auth-resolution surface and the retry shell are reference-implementation concerns, not Cookfile language. The §8.6 chapter is unchanged.

## 3. Architecture

### 3.1. Auth-key resolution simplified

Pre-CS-0059 (`resolved_api_key` at `cli/crates/cook-cache/src/cloud_config.rs:149-156`):

```text
COOK_CLOUD_API_KEY env var (non-empty)  ──┐
                                          ├─► Some(key) | None
[cloud] api_key in cloud.toml ────────────┘
```

Post-CS-0059:

```text
COOK_CLOUD_API_KEY env var (non-empty) ──► Some(key) | None
```

The TOML field is removed from `CloudSection`. Stray `api_key = "..."` lines in pre-CS-0059 cloud.toml files deserialise cleanly because `serde` ignores unknown fields by default — `CloudSection` carries no `#[serde(deny_unknown_fields)]`. A user upgrading sees the field silently ignored; resolution falls back to env-var-only and surfaces `MissingApiKey` if the env var is unset, prompting the user to migrate.

`MissingApiKey`'s diagnostic is rewritten to name only the env var and reference the future flow:

> `[cloud] enabled=true but no API key resolved — export COOK_CLOUD_API_KEY=<your-token> (interactive `cook cloud login` is planned in a future release)`

Validation in `load_or_default` is unchanged in shape — it still calls `resolved_api_key()` and errors with `MissingApiKey` when the result is `None`.

### 3.2. `Retry-After` honour on 429

The `BackendError::QuotaExceeded` unit variant becomes `QuotaExceeded(Option<Duration>)`:

- `Some(d)` — the server emitted a delta-seconds `Retry-After: <n>` header. The retry shell sleeps `d.clamp(backoff_initial, backoff_max)` and retries (still bounded by `BackendConfig::max_retries`). No jitter — the server told us when to come back; jitter would defeat the explicit hint.
- `None` — no parseable hint (header absent, or HTTP-date form). Terminal — preserves CS-0058 behaviour: log, drop the put, build continues.

The retry-shell change is one new `match` arm in `retry_loop`; the exponential `delay` cursor used by the `Transient` arm is **not** advanced by quota retries (the two backoff schedules are independent — a server-driven hint should not push out the next transient retry).

### 3.3. Where the parser lives

`parse_retry_after(response: &ureq::Response) -> Option<Duration>` is a new free function in `cloud_backend.rs`, alongside the existing `parse_content_hash`. It reads `response.header("Retry-After")`, trims whitespace, and parses as `u64`; on success returns `Some(Duration::from_secs(n))`, otherwise `None`. The parse-as-`u64` happily fails for HTTP-date strings, which is the desired fall-through.

Order matters in `map_ureq_error`: the `Retry-After` header read must happen **before** `response.into_string()` consumes the response. The refactor pulls the header read up.

## 4. Algorithms

### 4.1. Delta-seconds parser (recognised form)

```rust
fn parse_retry_after(response: &ureq::Response) -> Option<Duration> {
    response
        .header("Retry-After")?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}
```

`response.header()` returns `Option<&str>`; the `?` short-circuits to `None` when the header is absent. `parse::<u64>().ok()` discards parse errors (HTTP-date form, malformed integers, negatives). The whole pipeline is total — never panics, never errors.

### 4.2. Retry-shell quota arm

```rust
Err(BackendError::QuotaExceeded(Some(hint))) if attempt < config.max_retries => {
    let clamped = hint.clamp(config.backoff_initial, config.backoff_max);
    tracing::debug!(
        "cloud backend rate-limited (Retry-After={hint:?}); sleeping {clamped:?} \
         (attempt {}/{})",
        attempt + 1,
        config.max_retries + 1,
    );
    std::thread::sleep(clamped);
    attempt += 1;
    continue;
}
```

Three properties are pinned by the test plan:

- A `Retry-After: 1` hint with a generous `backoff_max = 5s` produces ≥ ~1s elapsed (the hint lands unclamped).
- A `Retry-After: 600` hint (10 minutes) with a tight `backoff_max = 50ms` produces ≤ a few hundred ms elapsed (clamped to the upper bound).
- A `Retry-After: <HTTP-date>` produces exactly one HTTP call (terminal — the parser falls through to `None`).

## 5. Backwards compatibility

- **Cloud-toml shape**: stray `[cloud] api_key = "..."` lines from pre-CS-0059 configs are silently ignored (serde default). Behaviour change: where pre-CS-0059 a user could authenticate purely from the TOML form, post-CS-0059 they cannot — the env var is mandatory. The `MissingApiKey` diagnostic is the migration prompt.
- **`BackendError::QuotaExceeded` Rust API**: changed from a unit variant to `QuotaExceeded(Option<Duration>)`. This is a breaking change for any out-of-tree code that pattern-matches on the enum. The cook workspace has zero such consumers today; in-tree call sites all live in `cli/crates/cook-cache/src/cloud_backend.rs` and are updated atomically.
- **Cache layer**: unchanged. `LocalBackend`, the trait surface other than the `BackendError` enum, the on-disk schema, and the wire protocol headers all stay put.

## 6. Test plan

Unit tests (`cli/crates/cook-cache/src/cloud_backend.rs::tests`):

- `cloud_backend_does_not_retry_on_429_without_retry_after` — renamed from the pre-CS-0059 `cloud_backend_does_not_retry_on_429`; asserts `QuotaExceeded(None)` when the server omits the header. Pins the CS-0058 terminal behaviour.
- `cloud_backend_honors_retry_after_on_429` — server emits 429 + `Retry-After: 1` then 200; assert exactly two HTTP calls and elapsed ≥ ~900ms.
- `cloud_backend_clamps_retry_after_to_backoff_max` — server emits 429 + `Retry-After: 600` then 200, with `backoff_max = 50ms`; assert elapsed < 2s.
- `cloud_backend_retry_after_http_date_falls_through_to_none` — server emits 429 + `Retry-After: <HTTP-date>`; assert exactly one HTTP call (terminal, parsed as `None`).

Unit tests (`cli/crates/cook-cache/src/cloud_config.rs::tests`):

- `cloud_enabled_with_project_ok` — refactored to use the env var (no TOML `api_key` line).
- `cloud_enabled_uses_env_var_api_key` — kept as-is; primary happy path.
- `cloud_enabled_requires_api_key` — kept as-is; surfaces `MissingApiKey` when env var is unset.
- `cloud_enabled_requires_endpoint` — refactored to set the env var so the `MissingEndpoint` branch is exercised independently of `MissingApiKey`.
- `cloud_empty_env_var_treated_as_unset` — renamed from `cloud_empty_env_var_falls_through_to_toml`. Asserts `MissingApiKey` for cloud-enabled configs and `None` resolution for cloud-disabled.
- `legacy_toml_api_key_field_silently_ignored` — new. Pins the migration story: pre-CS-0059 TOML files load without error; the field is just ignored.
- Deleted: `cloud_enabled_uses_toml_api_key`, `cloud_env_var_wins_over_toml` (no TOML form to assert against).

End-to-end fixtures (`examples/cache_benchmarks/verify.sh`, `examples/cache_dep_drift/verify.sh`) are unchanged — they exercise the `LocalBackend` path and never touch cloud auth or 429 handling.

## 7. Open questions

1. **HTTP-date support timeline.** v2 polish if a server we care about uses it. CF Rate Limiter and BetterAuth both emit delta-seconds, so this may stay deferred indefinitely.
2. **Per-endpoint backoff isolation.** A 429 on one endpoint currently informs the retry of the same endpoint only. A future shape could maintain a per-endpoint backoff cursor so a single hint suppresses concurrent calls to other endpoints; v1 has no such coordination.
3. **`Retry-After: 0`.** Means "you may retry immediately" per RFC 9110. CS-0059's clamp pins this to `backoff_initial`, which is the right safety net but slightly slower than the server intended. Acceptable; pin in the test if it ever matters.
