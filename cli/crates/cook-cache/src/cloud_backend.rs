//! `CloudBackend` — sync HTTP client implementing `CacheBackend` over a
//! v1 wire protocol against the Cook Cloud artifact server.
//!
//! See `standard/specs/2026-05-04-cache-cloud-backend-skeleton-design.md`
//! (CS-0058) for the protocol design — endpoints, header set, status code
//! mapping, retry policy.
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
        let url = self.artifact_url(key);
        let auth = self.auth_header();
        retry_loop(&self.config, || {
            let req = self.client.get(&url).set("Authorization", &auth);
            match req.call() {
                Ok(response) => {
                    let expected = parse_content_hash(&response)?;
                    let body = response.into_reader();
                    // Wrap in VerifyingReader — same fail-closed semantics
                    // as LocalBackend::get. Box<dyn Read + Send> is the
                    // trait return; the reader is Send because both inner
                    // (ureq's reader) and Sha256 are Send.
                    Ok(Some(Box::new(VerifyingReader::new(body, expected))
                        as Box<dyn Read + Send>))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn key(byte: u8) -> CloudKey {
        let mut k = [0u8; 32];
        k[0] = byte;
        k
    }

    fn sample_meta() -> ArtifactMeta {
        ArtifactMeta {
            recipe_namespace: "cook/Cookfile::build".into(),
            command_hash: 0,
            env_contribution: 0,
            schema_version: 3,
            size_bytes: 0,
            tags: BTreeSet::new(),
            consulted_env_keys: BTreeSet::new(),
            output_index: 0,
            output_path: "build/foo.o".into(),
            content_hash: ArtifactMeta::zero_content_hash(),
            kind: None,
        }
    }

    /// Build a backend with the given config pointed at the mockito
    /// server's URL. Tight backoff so retry tests don't drag.
    fn make_backend(server_url: &str, max_retries: u32) -> CloudBackend {
        let cfg = BackendConfig {
            timeout: Duration::from_secs(5),
            max_retries,
            backoff_initial: Duration::from_millis(1),
            backoff_max: Duration::from_millis(5),
            max_artifact_bytes: 1024 * 1024,
        };
        CloudBackend::new(server_url.to_string(), "test-token-zzz".into(), cfg)
    }

    #[test]
    fn cloud_backend_get_round_trips() {
        let mut server = mockito::Server::new();
        let bytes = b"hello cloud backend";
        let hash: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
        let k = key(0x10);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));

        let m = server
            .mock("GET", url_path.as_str())
            .match_header("authorization", "Bearer test-token-zzz")
            .with_status(200)
            .with_header("X-Cook-Content-Hash", &hex::encode(hash))
            .with_header("X-Cook-Size-Bytes", &bytes.len().to_string())
            .with_body(bytes)
            .create();

        let backend = make_backend(&server.url(), 0);
        let mut reader = backend.get(&k).expect("get").expect("hit");
        let mut out = Vec::new();
        reader.read_to_end(&mut out).expect("read_to_end");
        assert_eq!(out, bytes);
        m.assert();
    }

    #[test]
    fn cloud_backend_get_returns_none_on_404() {
        let mut server = mockito::Server::new();
        let k = key(0x11);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let m = server
            .mock("GET", url_path.as_str())
            .with_status(404)
            .create();

        let backend = make_backend(&server.url(), 0);
        let result = backend.get(&k).expect("get");
        assert!(result.is_none());
        m.assert();
    }

    #[test]
    fn cloud_backend_get_fails_closed_on_byte_tamper() {
        let mut server = mockito::Server::new();
        let bytes = b"the real bytes";
        // Hash for *different* bytes — guarantees mismatch.
        let bogus: [u8; 32] = <Sha256 as Digest>::digest(b"DIFFERENT bytes").into();
        let k = key(0x12);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));

        let _m = server
            .mock("GET", url_path.as_str())
            .with_status(200)
            .with_header("X-Cook-Content-Hash", &hex::encode(bogus))
            .with_body(bytes)
            .create();

        let backend = make_backend(&server.url(), 0);
        let mut reader = backend.get(&k).expect("get").expect("hit");
        let mut out = Vec::new();
        let err = reader
            .read_to_end(&mut out)
            .expect_err("VerifyingReader must surface InvalidData on hash mismatch");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn cloud_backend_put_round_trips() {
        let mut server = mockito::Server::new();
        let k = key(0x20);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let payload = b"some bytes to upload";

        let m = server
            .mock("PUT", url_path.as_str())
            .match_header("authorization", "Bearer test-token-zzz")
            .match_body(mockito::Matcher::Exact(
                String::from_utf8(payload.to_vec()).unwrap(),
            ))
            .with_status(200)
            .create();

        let backend = make_backend(&server.url(), 0);
        let mut meta = sample_meta();
        meta.size_bytes = payload.len() as u64;
        let mut cursor = std::io::Cursor::new(payload.to_vec());
        backend.put(&k, &mut cursor, &mut meta).expect("put");
        m.assert();
    }

    #[test]
    fn cloud_backend_put_rejects_oversize() {
        // The cap is enforced client-side BEFORE the request completes.
        // Mockito may still observe a partial connection; use `expect_at_most`
        // to avoid coupling the test to the abort timing.
        let mut server = mockito::Server::new();
        let k = key(0x21);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let _m = server
            .mock("PUT", url_path.as_str())
            .with_status(200)
            .expect_at_most(1)
            .create();

        let cfg = BackendConfig {
            timeout: Duration::from_secs(5),
            max_retries: 0,
            backoff_initial: Duration::from_millis(1),
            backoff_max: Duration::from_millis(5),
            max_artifact_bytes: 100, // small cap
        };
        let backend = CloudBackend::new(server.url(), "test-token-zzz".into(), cfg);
        let payload = vec![0xABu8; 500]; // 5x the cap
        let mut meta = sample_meta();
        let mut cursor = std::io::Cursor::new(payload);
        let err = backend
            .put(&k, &mut cursor, &mut meta)
            .expect_err("oversize put must error");
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds"),
            "diagnostic must mention 'exceeds'; got: {msg}"
        );
        assert!(
            msg.contains("100"),
            "diagnostic must name the cap (100); got: {msg}"
        );
    }

    #[test]
    fn cloud_backend_put_handles_409_conflict() {
        let mut server = mockito::Server::new();
        let k = key(0x22);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let m = server
            .mock("PUT", url_path.as_str())
            .with_status(409)
            .with_body("server-side bytes differ")
            .create();

        let backend = make_backend(&server.url(), 0);
        let mut meta = sample_meta();
        let mut cursor = std::io::Cursor::new(b"new bytes".to_vec());
        let err = backend
            .put(&k, &mut cursor, &mut meta)
            .expect_err("409 must error");
        let msg = err.to_string();
        assert!(
            msg.contains("conflict"),
            "diagnostic must mention 'conflict'; got: {msg}"
        );
        match err {
            BackendError::Other(_) => {}
            other => panic!("expected BackendError::Other, got {other:?}"),
        }
        m.assert();
    }

    #[test]
    fn cloud_backend_batch_query_round_trips() {
        let mut server = mockito::Server::new();
        let k1 = key(0x30);
        let k2 = key(0x31);
        let k3 = key(0x32);
        // Server says only k1 and k3 are present.
        let response_body = serde_json::json!({
            "present": [hex::encode(k1), hex::encode(k3)]
        })
        .to_string();

        let m = server
            .mock("POST", "/v1/artifacts/batch_query")
            .match_header("authorization", "Bearer test-token-zzz")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response_body)
            .create();

        let backend = make_backend(&server.url(), 0);
        let hits = backend.batch_query(&[k1, k2, k3]).expect("batch_query");
        assert!(hits.contains(&k1));
        assert!(!hits.contains(&k2));
        assert!(hits.contains(&k3));
        m.assert();
    }

    #[test]
    fn cloud_backend_retries_on_5xx() {
        let mut server = mockito::Server::new();
        let k = key(0x40);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        // First call: 503. Second call: 404 (= Ok(None)).
        // Use create() ordering — mockito returns mocks in registration
        // order until each is exhausted.
        let m_503 = server
            .mock("GET", url_path.as_str())
            .with_status(503)
            .expect(1)
            .create();
        let m_404 = server
            .mock("GET", url_path.as_str())
            .with_status(404)
            .expect(1)
            .create();

        let backend = make_backend(&server.url(), 3);
        let result = backend.get(&k).expect("get");
        assert!(result.is_none(), "second call should be 404 → Ok(None)");
        m_503.assert();
        m_404.assert();
    }

    #[test]
    fn cloud_backend_does_not_retry_on_401() {
        let mut server = mockito::Server::new();
        let k = key(0x41);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let m = server
            .mock("GET", url_path.as_str())
            .with_status(401)
            .expect(1) // exactly one call — no retry on auth failure
            .create();

        let backend = make_backend(&server.url(), 5);
        match backend.get(&k) {
            Err(BackendError::Unauthorized(_)) => {}
            Err(other) => panic!("expected BackendError::Unauthorized, got {other:?}"),
            Ok(_) => panic!("expected error, got success"),
        }
        m.assert();
    }

    /// CS-0058: a 429 with NO `Retry-After` header is terminal. The retry
    /// shell sees `QuotaExceeded(None)` and falls through. CS-0059
    /// preserves this — the `None` payload is the "no hint" sentinel.
    #[test]
    fn cloud_backend_does_not_retry_on_429_without_retry_after() {
        let mut server = mockito::Server::new();
        let k = key(0x42);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let m = server
            .mock("GET", url_path.as_str())
            .with_status(429)
            .expect(1)
            .create();

        let backend = make_backend(&server.url(), 5);
        match backend.get(&k) {
            Err(BackendError::QuotaExceeded(None)) => {}
            Err(other) => panic!("expected BackendError::QuotaExceeded(None), got {other:?}"),
            Ok(_) => panic!("expected error, got success"),
        }
        m.assert();
    }

    /// CS-0059: a 429 WITH `Retry-After: <delta-seconds>` is retryable.
    /// The retry shell sleeps the server-supplied hint (clamped to
    /// `[backoff_initial, backoff_max]`), then retries. Server returns
    /// 429 on call 1 and 200 on call 2; the test asserts exactly two
    /// calls happened and that elapsed time is at least the hinted delay.
    #[test]
    fn cloud_backend_honors_retry_after_on_429() {
        use std::time::Instant;

        let mut server = mockito::Server::new();
        let bytes = b"";
        let hash: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
        let k = key(0x43);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));

        // Call 1: 429 with Retry-After: 1 (one second).
        let m_429 = server
            .mock("GET", url_path.as_str())
            .with_status(429)
            .with_header("Retry-After", "1")
            .expect(1)
            .create();
        // Call 2: 200 with valid headers and empty body.
        let m_200 = server
            .mock("GET", url_path.as_str())
            .with_status(200)
            .with_header("X-Cook-Content-Hash", &hex::encode(hash))
            .with_header("X-Cook-Size-Bytes", "0")
            .with_header("X-Cook-Schema-Version", "3")
            .with_header("X-Cook-Recipe-Namespace", "cook/Cookfile::build")
            .with_header("X-Cook-Output-Index", "0")
            .with_header("X-Cook-Output-Path", "build/foo.o")
            .with_body(bytes)
            .expect(1)
            .create();

        // backoff_max = 5s gives the 1s hint room to land unclamped.
        let cfg = BackendConfig {
            timeout: Duration::from_secs(5),
            max_retries: 3,
            backoff_initial: Duration::from_millis(1),
            backoff_max: Duration::from_secs(5),
            max_artifact_bytes: 1024 * 1024,
        };
        let backend = CloudBackend::new(server.url(), "test-token-zzz".into(), cfg);

        let started = Instant::now();
        let mut reader = backend.get(&k).expect("get ok").expect("present");
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).expect("read body");
        let elapsed = started.elapsed();

        assert_eq!(buf, bytes);
        assert!(
            elapsed >= Duration::from_millis(900),
            "Retry-After=1s must produce at least ~1s elapsed, got {elapsed:?}"
        );
        m_429.assert();
        m_200.assert();
    }

    /// CS-0059: a `Retry-After` hint that exceeds `backoff_max` is
    /// clamped down. Server returns 429 with `Retry-After: 600` (10 min)
    /// and a tight `backoff_max = 50ms`. The retry must proceed within a
    /// few hundred ms — far below the 10-minute hint.
    #[test]
    fn cloud_backend_clamps_retry_after_to_backoff_max() {
        use std::time::Instant;

        let mut server = mockito::Server::new();
        let bytes = b"";
        let hash: [u8; 32] = <Sha256 as Digest>::digest(bytes).into();
        let k = key(0x44);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));

        let m_429 = server
            .mock("GET", url_path.as_str())
            .with_status(429)
            .with_header("Retry-After", "600")
            .expect(1)
            .create();
        let m_200 = server
            .mock("GET", url_path.as_str())
            .with_status(200)
            .with_header("X-Cook-Content-Hash", &hex::encode(hash))
            .with_header("X-Cook-Size-Bytes", "0")
            .with_header("X-Cook-Schema-Version", "3")
            .with_header("X-Cook-Recipe-Namespace", "cook/Cookfile::build")
            .with_header("X-Cook-Output-Index", "0")
            .with_header("X-Cook-Output-Path", "build/foo.o")
            .with_body(bytes)
            .expect(1)
            .create();

        let cfg = BackendConfig {
            timeout: Duration::from_secs(5),
            max_retries: 3,
            backoff_initial: Duration::from_millis(1),
            backoff_max: Duration::from_millis(50),
            max_artifact_bytes: 1024 * 1024,
        };
        let backend = CloudBackend::new(server.url(), "test-token-zzz".into(), cfg);

        let started = Instant::now();
        let mut reader = backend.get(&k).expect("get ok").expect("present");
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).expect("read body");
        let elapsed = started.elapsed();

        assert_eq!(buf, bytes);
        // Generous upper bound — even on a slow CI runner, 10 minutes of
        // unclamped sleep would blow this by orders of magnitude.
        assert!(
            elapsed < Duration::from_secs(2),
            "Retry-After=600s must clamp to backoff_max=50ms, got {elapsed:?}"
        );
        m_429.assert();
        m_200.assert();
    }

    /// CS-0059: HTTP-date form of `Retry-After` is recognised by the
    /// parser but not honoured (delta-seconds only in v1). Maps to
    /// `QuotaExceeded(None)` → terminal, no retry. Pins the parser's
    /// fall-through behaviour.
    #[test]
    fn cloud_backend_retry_after_http_date_falls_through_to_none() {
        let mut server = mockito::Server::new();
        let k = key(0x45);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let m = server
            .mock("GET", url_path.as_str())
            .with_status(429)
            .with_header("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT")
            .expect(1)
            .create();

        let backend = make_backend(&server.url(), 5);
        match backend.get(&k) {
            Err(BackendError::QuotaExceeded(None)) => {}
            Err(other) => panic!(
                "HTTP-date form must map to QuotaExceeded(None) (terminal), got {other:?}"
            ),
            Ok(_) => panic!("expected error, got success"),
        }
        m.assert();
    }

    #[test]
    fn cloud_backend_health_returns_ok_on_200() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/health")
            .match_header("authorization", "Bearer test-token-zzz")
            .with_status(200)
            .create();

        let backend = make_backend(&server.url(), 0);
        backend.health().expect("health ok");
        m.assert();
    }

    #[test]
    fn cloud_backend_health_returns_transient_on_5xx() {
        let mut server = mockito::Server::new();
        // All 4 calls (initial + 3 retries) return 503; then we exhaust
        // retries and surface Transient.
        let m = server
            .mock("GET", "/v1/health")
            .with_status(503)
            .expect(4)
            .create();

        let backend = make_backend(&server.url(), 3);
        let err = backend.health().expect_err("5xx must error after retries");
        match err {
            BackendError::Transient(_) => {}
            other => panic!("expected BackendError::Transient, got {other:?}"),
        }
        m.assert();
    }

    #[test]
    fn cloud_backend_delete_204_succeeds() {
        let mut server = mockito::Server::new();
        let k = key(0x50);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let m = server
            .mock("DELETE", url_path.as_str())
            .with_status(204)
            .create();

        let backend = make_backend(&server.url(), 0);
        backend.delete(&k).expect("delete ok");
        m.assert();
    }

    #[test]
    fn cloud_backend_delete_404_idempotent() {
        let mut server = mockito::Server::new();
        let k = key(0x51);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        let m = server
            .mock("DELETE", url_path.as_str())
            .with_status(404)
            .create();

        let backend = make_backend(&server.url(), 0);
        backend.delete(&k).expect("delete missing must be idempotent");
        m.assert();
    }

    #[test]
    fn cloud_backend_get_errors_on_missing_content_hash_header() {
        let mut server = mockito::Server::new();
        let k = key(0x60);
        let url_path = format!("/v1/artifacts/{}", hex::encode(k));
        // 200 OK but no X-Cook-Content-Hash — server misbehaviour.
        let _m = server
            .mock("GET", url_path.as_str())
            .with_status(200)
            .with_body(b"some bytes" as &[u8])
            .create();

        let backend = make_backend(&server.url(), 0);
        match backend.get(&k) {
            Err(BackendError::Other(msg)) => {
                assert!(
                    msg.contains("X-Cook-Content-Hash"),
                    "diagnostic must mention the missing header; got: {msg}"
                );
            }
            Err(other) => panic!("expected BackendError::Other, got {other:?}"),
            Ok(_) => panic!("expected error, got success"),
        }
    }

    #[test]
    fn jitter_factor_in_range() {
        // 50 samples should all fall in [0.75, 1.25].
        for _ in 0..50 {
            let f = jitter_factor();
            assert!(f >= 0.75 && f <= 1.25, "jitter factor out of range: {f}");
            // Tiny sleep to advance the clock for differentiated samples.
            std::thread::sleep(Duration::from_micros(1));
        }
    }
}
