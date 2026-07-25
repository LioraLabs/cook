//! `CloudBackend` — sync HTTP client implementing `CacheBackend` over a
//! v1 wire protocol against the Cook Cloud artifact server.
//!
//! ## Wire protocol summary
//!
//! All paths versioned under `/v1/`. Bearer-token auth on every request.
//!
//! - `POST /v1/artifacts/batch_query` — JSON body `{keys: [hex...]}`,
//!   response `{present: [hex...]}`.
//! - `GET /v1/artifacts/{key_hex}` — bytes streamed; meta in `X-Cook-*`
//!   headers; response wrapped in a `VerifyingReader` keyed on
//!   `X-Cook-Content-Hash`.
//! - `PUT /v1/artifacts/{key_hex}` — bytes streamed from caller; same
//!   `X-Cook-*` headers; client enforces `max_artifact_bytes` mid-stream.
//! - `DELETE /v1/artifacts/{key_hex}` — 204 success, 404 idempotent success.
//! - `GET /v1/health` — 200 healthy, anything else `Transient`.
//!
//! ## Retry policy (CS-0057 `BackendConfig`)
//!
//! Only `BackendError::Transient` (5xx + network errors) is retried.
//! `Unauthorized`, `QuotaExceeded`, and `Other` (including 409 conflict
//! and 413 oversize) return immediately. Backoff is jittered exponential:
//! `delay = backoff_initial`, doubled each retry, capped at `backoff_max`,
//! with ±25% uniform jitter applied to each sleep.

use std::collections::BTreeSet;
use std::io::{self, Read};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::backend::VerifyingReader;
use cook_fingerprint::backend::{
    ArtifactMeta, BackendConfig, BackendError, BackendResult, CacheBackend, CloudKey,
    DeterminantManifest,
};

/// Sync HTTP client implementing `CacheBackend` against a remote artifact
/// server (Cook Cloud). Constructed once per build; thread-safe via
/// `ureq::Agent`'s internal pool.
pub struct CloudBackend {
    /// Base URL — e.g. `"https://api.cook.dev"`. The client appends
    /// `/v1/...` for every request.
    endpoint: String,
    /// Bearer token sent as `Authorization: Bearer <api_key>` on every
    /// request. NEVER logged, NEVER surfaced in diagnostics.
    api_key: String,
    /// HTTP agent. Has the per-call timeout from `config.timeout` baked in
    /// at construction.
    client: ureq::Agent,
    /// CS-0057 tunables. `timeout` was already consumed by the agent;
    /// `max_retries`, `backoff_initial`, `backoff_max` drive the retry
    /// shell; `max_artifact_bytes` is enforced in `put`.
    config: BackendConfig,
}

impl CloudBackend {
    /// Construct a `CloudBackend`. Builds a `ureq::Agent` with
    /// `config.timeout` as the per-call timeout. Trailing slash on
    /// `endpoint` is stripped to keep URL composition trivial.
    pub fn new(endpoint: String, api_key: String, config: BackendConfig) -> Self {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        let client = ureq::AgentBuilder::new()
            .timeout(config.timeout)
            .build();
        Self {
            endpoint,
            api_key,
            client,
            config,
        }
    }

    /// Borrow the active `BackendConfig`. Diagnostic accessor for tests
    /// and observability call sites; not part of the `CacheBackend` trait.
    pub fn config(&self) -> &BackendConfig {
        &self.config
    }

    /// Compose the URL for an artifact key.
    fn artifact_url(&self, key: &CloudKey) -> String {
        format!("{}/v1/artifacts/{}", self.endpoint, hex::encode(key))
    }

    /// Compose the URL for the batch_query endpoint.
    fn batch_query_url(&self) -> String {
        format!("{}/v1/artifacts/batch_query", self.endpoint)
    }

    /// Compose the URL for the health endpoint.
    fn health_url(&self) -> String {
        format!("{}/v1/health", self.endpoint)
    }

    /// Authorization header value. Never logged.
    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }
}

// ─── helpers: status mapping, retry, jitter, header packing ───────────────

/// CS-0059. Parse a `Retry-After` response header per RFC 9110 §10.2.3.
/// v1 supports the **delta-seconds** form only — an integer count of
/// seconds the client SHOULD wait before retrying. The HTTP-date form is
/// recognised by the parser but treated as `None` (no hint) because v1's
/// retry shell sleeps a `Duration`, not a wall-clock target, and timezone
/// /clock-skew handling is out of scope. CF Rate Limiter and BetterAuth's
/// rate-limit middleware emit delta-seconds, so this is the form we'll
/// see in practice; HTTP-date support can be added in a future revision
/// if a server we care about uses it.
fn parse_retry_after(response: &ureq::Response) -> Option<Duration> {
    response
        .header("Retry-After")?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Map an HTTP status code to a `BackendError` variant. The body diagnostic
/// is included; the request's `Authorization` header is NEVER included
/// (the body is server-supplied, not request-derived).
///
/// `retry_after` is the server-supplied hint parsed from the `Retry-After`
/// header (if any); it's only meaningful for `429` but threaded uniformly
/// to keep the call sites simple.
fn map_status_error(
    status: u16,
    ctx: &str,
    body: String,
    retry_after: Option<Duration>,
) -> BackendError {
    match status {
        401 | 403 => BackendError::Unauthorized(format!("{ctx}: status {status}: {body}")),
        429 => BackendError::QuotaExceeded(retry_after),
        500..=599 => BackendError::Transient(format!("{ctx}: status {status}: {body}")),
        409 => BackendError::Other(format!(
            "conflict at {ctx}: server-side bytes differ: {body}"
        )),
        413 => BackendError::Other(format!(
            "server rejected oversize artifact at {ctx}: {body}"
        )),
        400 => BackendError::Other(format!("bad request at {ctx}: {body}")),
        // 404 is caller-handled (get → Ok(None), delete → Ok(())); if we
        // reach this mapper with 404 it's an unexpected location.
        _ => BackendError::Other(format!("{ctx}: unexpected status {status}: {body}")),
    }
}

/// Map a `ureq::Error` to a `BackendError`. `ureq::Error::Status` carries
/// an HTTP status; `ureq::Error::Transport` is a network/IO failure
/// (always `Transient`).
///
/// CS-0059: extract `Retry-After` from response headers BEFORE consuming
/// the body via `into_string()` — the response is moved into the
/// body-extract call, so any header read must happen first.
fn map_ureq_error(err: ureq::Error, ctx: &str) -> BackendError {
    match err {
        ureq::Error::Status(status, response) => {
            let retry_after = parse_retry_after(&response);
            let body = response.into_string().unwrap_or_else(|_| "<no body>".into());
            map_status_error(status, ctx, body, retry_after)
        }
        ureq::Error::Transport(t) => {
            BackendError::Transient(format!("{ctx}: transport: {t}"))
        }
    }
}

/// Pseudo-random ±25% jitter factor in `[0.75, 1.25]`. Uses the system
/// clock's nanosecond field as entropy — sufficient for thundering-herd
/// breakage; not cryptographic.
fn jitter_factor() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    // Mix the nanos with a simple hash so successive close-spaced calls
    // don't produce closely-correlated factors.
    let mixed = nanos
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xBF58_476D_1CE4_E5B9);
    let unit = (mixed as f64) / (u64::MAX as f64); // [0.0, 1.0]
    0.75 + 0.5 * unit
}

/// Apply jitter and cap to a backoff delay.
fn jittered_capped(delay: Duration, cap: Duration) -> Duration {
    let nanos = delay.as_nanos() as f64 * jitter_factor();
    let jittered = Duration::from_nanos(nanos as u64);
    if jittered > cap {
        cap
    } else {
        jittered
    }
}

/// Retry shell. Calls `op` up to `1 + max_retries` times, retrying on:
///
/// - `BackendError::Transient` — sleeps the exponentially-growing
///   `backoff_initial → backoff_max` schedule with ±25% jitter.
/// - `BackendError::QuotaExceeded(Some(hint))` (CS-0059) — sleeps the
///   server-supplied `hint` clamped to `[backoff_initial, backoff_max]`,
///   no jitter (the server told us when to come back; the bounds keep us
///   from sleeping forever or hammering immediately). Does NOT advance
///   the exponential `delay` cursor — quota retries are independent of
///   the transient-error backoff schedule.
///
/// `BackendError::QuotaExceeded(None)` is terminal (CS-0058 behaviour
/// preserved when the server omits the header). All other variants
/// terminate immediately.
fn retry_loop<T, F>(config: &BackendConfig, mut op: F) -> BackendResult<T>
where
    F: FnMut() -> BackendResult<T>,
{
    let mut attempt: u32 = 0;
    let mut delay = config.backoff_initial;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(BackendError::Transient(msg)) if attempt < config.max_retries => {
                tracing::debug!(
                    "cloud backend transient (attempt {}/{}): {msg}; sleeping {delay:?}",
                    attempt + 1,
                    config.max_retries + 1,
                );
                let sleep_for = jittered_capped(delay, config.backoff_max);
                std::thread::sleep(sleep_for);
                // Double the next base delay, capped.
                delay = std::cmp::min(delay.saturating_mul(2), config.backoff_max);
                attempt += 1;
                continue;
            }
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
            Err(e) => return Err(e),
        }
    }
}

// ─── batch_query JSON shapes ───────────────────────────────────────────────

#[derive(Serialize)]
struct BatchQueryRequest<'a> {
    keys: Vec<&'a str>,
}

#[derive(Deserialize)]
struct BatchQueryResponse {
    present: Vec<String>,
}

// ─── put: counting + size-capping reader ──────────────────────────────────

/// Wraps a caller's `&mut dyn Read` and aborts (via `io::Error`) when more
/// than `limit` bytes have been read. Used by `put` to enforce
/// `max_artifact_bytes` mid-stream — same shape as `LocalBackend::put`'s
/// inner-loop check, but expressed as a reader so it composes with
/// `ureq::Request::send(reader)`.
struct CappedReader<'a> {
    inner: &'a mut dyn Read,
    total: u64,
    limit: u64,
    /// Sticky flag — once we've raised the cap-exceeded error, every
    /// subsequent read returns 0 (EOF). `ureq` may call `read` once more
    /// after we error; this avoids re-erroring.
    done: bool,
}

impl<'a> CappedReader<'a> {
    fn new(inner: &'a mut dyn Read, limit: u64) -> Self {
        Self {
            inner,
            total: 0,
            limit,
            done: false,
        }
    }
}

impl<'a> Read for CappedReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        let n = self.inner.read(buf)?;
        if n == 0 {
            return Ok(0);
        }
        self.total = self.total.saturating_add(n as u64);
        if self.total > self.limit {
            self.done = true;
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "artifact exceeds max_artifact_bytes ({}); cap {}",
                    self.total, self.limit
                ),
            ));
        }
        Ok(n)
    }
}

// ─── header packing / parsing ─────────────────────────────────────────────

/// Apply the `X-Cook-*` meta headers + `Authorization` to a request
/// builder. Used by `put`. The meta is the caller's; `content_hash` may be
/// the zero sentinel.
fn put_headers(req: ureq::Request, auth: &str, meta: &ArtifactMeta) -> ureq::Request {
    req.set("Authorization", auth)
        .set("X-Cook-Content-Hash", &hex::encode(meta.content_hash))
        .set("X-Cook-Size-Bytes", &meta.size_bytes.to_string())
        .set("X-Cook-Schema-Version", &meta.schema_version.to_string())
        .set("X-Cook-Recipe-Namespace", &meta.recipe_namespace)
        .set("X-Cook-Output-Index", &meta.output_index.to_string())
        .set("X-Cook-Output-Path", &meta.output_path)
        .set("X-Cook-Mode", &meta.mode.to_string())
        .set("X-Cook-Kind", meta.kind.as_deref().unwrap_or(""))
        .set("X-Cook-Symlink-Target", meta.target.as_deref().unwrap_or(""))
}

/// Parse `X-Cook-Content-Hash` from a response. The header is REQUIRED on
/// `200 OK` per CS-0058 §3.2.4; missing or malformed → `Other`.
fn parse_content_hash(response: &ureq::Response) -> BackendResult<[u8; 32]> {
    let h = response
        .header("X-Cook-Content-Hash")
        .ok_or_else(|| {
            BackendError::Other(
                "malformed response: missing X-Cook-Content-Hash header".into(),
            )
        })?;
    let mut out = [0u8; 32];
    hex::decode_to_slice(h, &mut out).map_err(|e| {
        BackendError::Other(format!(
            "malformed response: X-Cook-Content-Hash not 64-char hex: {e}"
        ))
    })?;
    Ok(out)
}

/// Reconstruct an `ArtifactMeta` from the `X-Cook-*` response headers in a
/// single round-trip. Called by `get_with_meta` before the body is consumed.
///
/// `content_hash` is REQUIRED (via `parse_content_hash`). All other fields
/// default gracefully: `mode` → `ArtifactMeta::default_mode()` (0o644),
/// `kind`/`target` → `None`, numeric fields → 0, string fields → "".
/// Fields not transmitted as headers (`command_hash`, `env_contribution`,
/// `seal_contribution`, `tags`, `consulted_env_keys`) use zero / empty-set
/// defaults — they are diagnostic and not needed for restore.
fn parse_meta(response: &ureq::Response) -> BackendResult<ArtifactMeta> {
    let content_hash = parse_content_hash(response)?;

    let mode = response
        .header("X-Cook-Mode")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or_else(ArtifactMeta::default_mode);

    let kind = response
        .header("X-Cook-Kind")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let target = response
        .header("X-Cook-Symlink-Target")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let recipe_namespace = response
        .header("X-Cook-Recipe-Namespace")
        .unwrap_or("")
        .to_string();

    let size_bytes = response
        .header("X-Cook-Size-Bytes")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let schema_version = response
        .header("X-Cook-Schema-Version")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    let output_index = response
        .header("X-Cook-Output-Index")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    let output_path = response
        .header("X-Cook-Output-Path")
        .unwrap_or("")
        .to_string();

    Ok(ArtifactMeta {
        recipe_namespace,
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version,
        size_bytes,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index,
        output_path,
        content_hash,
        kind,
        mode,
        target,
    })
}

// ─── trait impl ───────────────────────────────────────────────────────────

impl CacheBackend for CloudBackend {
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<BTreeSet<CloudKey>> {
        let url = self.batch_query_url();
        let auth = self.auth_header();
        let hex_keys: Vec<String> = keys.iter().map(hex::encode).collect();
        retry_loop(&self.config, || {
            let req = self
                .client
                .post(&url)
                .set("Authorization", &auth)
                .set("Content-Type", "application/json");
            let body = BatchQueryRequest {
                keys: hex_keys.iter().map(|s| s.as_str()).collect(),
            };
            let response = req
                .send_json(serde_json::to_value(&body).map_err(|e| {
                    BackendError::Other(format!("serialize batch_query body: {e}"))
                })?)
                .map_err(|e| map_ureq_error(e, "batch_query"))?;
            let parsed: BatchQueryResponse = response
                .into_json()
                .map_err(|e| BackendError::Other(format!("parse batch_query response: {e}")))?;
            let mut out: BTreeSet<CloudKey> = BTreeSet::new();
            for s in parsed.present {
                let mut k = [0u8; 32];
                hex::decode_to_slice(&s, &mut k).map_err(|e| {
                    BackendError::Other(format!(
                        "batch_query response: present[*] not 64-char hex: {e}"
                    ))
                })?;
                out.insert(k);
            }
            Ok(out)
        })
    }

    fn get(&self, key: &CloudKey) -> BackendResult<Option<Box<dyn Read + Send>>> {
        Ok(self.get_with_meta(key)?.map(|(r, _)| r))
    }

    fn get_with_meta(
        &self,
        key: &CloudKey,
    ) -> BackendResult<Option<(Box<dyn Read + Send>, ArtifactMeta)>> {
        // COOK-233 — deliberately no last-access touch here: there is no
        // local blob to stamp, and last-access for a remote entry is
        // server-managed. Only `LocalBackend` tracks it.
        let url = self.artifact_url(key);
        let auth = self.auth_header();
        retry_loop(&self.config, || {
            let req = self.client.get(&url).set("Authorization", &auth);
            match req.call() {
                Ok(response) => {
                    // parse_meta borrows &response (headers), then into_reader
                    // consumes it — both happen sequentially so the borrow ends
                    // before the move. This is the single-round-trip design: meta
                    // rides the same GET response headers as the body.
                    let meta = parse_meta(&response)?;
                    let body = response.into_reader();
                    Ok(Some((
                        Box::new(VerifyingReader::new(body, meta.content_hash))
                            as Box<dyn Read + Send>,
                        meta,
                    )))
                }
                Err(ureq::Error::Status(404, _)) => Ok(None),
                Err(e) => Err(map_ureq_error(e, "get")),
            }
        })
    }

    fn put(
        &self,
        key: &CloudKey,
        reader: &mut dyn Read,
        meta: &mut ArtifactMeta,
    ) -> BackendResult<()> {
        let url = self.artifact_url(key);
        let auth = self.auth_header();
        let limit = self.config.max_artifact_bytes;
        // NOTE: `reader` is not retryable — we cannot re-read its bytes.
        // For consistency with LocalBackend::put which also doesn't retry
        // its single-pass write, the cloud put issues exactly one HTTP
        // call. A 5xx surfaces as `Transient` to the caller; the engine
        // currently treats put failures as drop-and-continue, so retry
        // loss is observability-only.
        let req = self.client.put(&url);
        let req = put_headers(req, &auth, meta);

        let mut capped = CappedReader::new(reader, limit);
        match req.send(&mut capped) {
            Ok(_response) => Ok(()),
            Err(ureq::Error::Transport(t)) => {
                // The CappedReader's io::Error("artifact exceeds ...")
                // surfaces here as a transport error. Distinguish by
                // checking the `total > limit` state.
                if capped.done && capped.total > capped.limit {
                    Err(BackendError::Other(format!(
                        "artifact exceeds max_artifact_bytes ({}); cap {}",
                        capped.total, capped.limit
                    )))
                } else {
                    Err(BackendError::Transient(format!("put: transport: {t}")))
                }
            }
            Err(e) => Err(map_ureq_error(e, "put")),
        }
    }

    fn delete(&self, key: &CloudKey) -> BackendResult<()> {
        let url = self.artifact_url(key);
        let auth = self.auth_header();
        retry_loop(&self.config, || {
            match self.client.delete(&url).set("Authorization", &auth).call() {
                Ok(_) => Ok(()),
                // Idempotent: 404 means already absent. Trait contract
                // says delete-of-missing returns Ok(()).
                Err(ureq::Error::Status(404, _)) => Ok(()),
                Err(e) => Err(map_ureq_error(e, "delete")),
            }
        })
    }

    fn health(&self) -> BackendResult<()> {
        let url = self.health_url();
        let auth = self.auth_header();
        retry_loop(&self.config, || {
            match self.client.get(&url).set("Authorization", &auth).call() {
                Ok(_) => Ok(()),
                Err(e) => Err(map_ureq_error(e, "health")),
            }
        })
    }

    /// COOK-166 / CS-0110: the determinant manifest is local-filesystem
    /// diagnostic data (a `provenance.json` sidecar). The cloud backend has
    /// no manifest endpoint yet (producer-attestation upload is deferred to
    /// M2); this is a no-op so a manifest never blocks a build.
    fn put_manifest(
        &self,
        _key: &CloudKey,
        _manifest: &DeterminantManifest,
    ) -> BackendResult<()> {
        Ok(())
    }

    /// No manifest endpoint yet — a cloud miss is the absence case, never an
    /// error.
    fn get_manifest(&self, _key: &CloudKey) -> BackendResult<Option<DeterminantManifest>> {
        Ok(None)
    }
}

#[cfg(test)]
#[path = "tests/cloud_backend_tests.rs"]
mod tests;
