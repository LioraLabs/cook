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
//! - `GET /v1/manifests/{key_hex}` — the unit's `DeterminantManifest` as
//!   JSON, or 404 (also what a server without the route answers).
//! - `PUT /v1/manifests/{key_hex}` — JSON `DeterminantManifest` whose `key`
//!   is `{key_hex}`; last writer wins (no 409).
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
use crate::manifest_wire::{decode_manifest, ManifestRefusal, MANIFESTS_PATH};
use crate::backend::{
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

    /// Compose the URL for a unit's determinant manifest.
    fn manifest_url(&self, key: &CloudKey) -> String {
        format!("{}{MANIFESTS_PATH}{}", self.endpoint, hex::encode(key))
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
        // Disk full on the server: re-sending the body cannot help.
        507 => BackendError::Other(format!("{ctx}: server storage full (507): {body}")),
        500..=599 => BackendError::Transient(format!("{ctx}: status {status}: {body}")),
        409 => BackendError::Other(format!(
            "conflict at {ctx}: server-side bytes differ: {body}"
        )),
        413 if ctx == "batch_query" => BackendError::Other(format!(
            "server rejected oversize batch_query request: {body}"
        )),
        413 if ctx == "put_manifest" => BackendError::Other(format!(
            "server rejected oversize manifest at {ctx}: {body}"
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
    /// COOK-417: the accumulate-and-check, shared with `LocalBackend::put`.
    /// This file used to carry its own copy and concede it in a comment.
    cap: crate::cap::CapCounter,
    /// Sticky flag — once we've raised the cap-exceeded error, every
    /// subsequent read returns 0 (EOF). `ureq` may call `read` once more
    /// after we error; this avoids re-erroring.
    done: bool,
}

impl<'a> CappedReader<'a> {
    fn new(inner: &'a mut dyn Read, limit: u64) -> Self {
        Self {
            inner,
            cap: crate::cap::CapCounter::new(limit),
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
        if let Err(msg) = self.cap.add(n as u64) {
            self.done = true;
            return Err(io::Error::other(msg));
        }
        Ok(n)
    }
}

// ─── header codec (client and server share it) ────────────────────────────

/// Env var holding the bearer token — the client sends it, `cook cache serve`
/// requires it.
pub const API_KEY_ENV: &str = "COOK_CLOUD_API_KEY";

pub(crate) const HDR_CONTENT_HASH: &str = "X-Cook-Content-Hash";
pub(crate) const HDR_SIZE_BYTES: &str = "X-Cook-Size-Bytes";
pub(crate) const HDR_SCHEMA_VERSION: &str = "X-Cook-Schema-Version";
pub(crate) const HDR_RECIPE_NAMESPACE: &str = "X-Cook-Recipe-Namespace";
pub(crate) const HDR_OUTPUT_INDEX: &str = "X-Cook-Output-Index";
pub(crate) const HDR_OUTPUT_PATH: &str = "X-Cook-Output-Path";
pub(crate) const HDR_MODE: &str = "X-Cook-Mode";
pub(crate) const HDR_KIND: &str = "X-Cook-Kind";
pub(crate) const HDR_SYMLINK_TARGET: &str = "X-Cook-Symlink-Target";

/// The `X-Cook-*` headers carrying an artifact's meta. `content_hash` may be
/// the zero sentinel (a put whose hash the receiver computes).
pub(crate) fn meta_to_headers(meta: &ArtifactMeta) -> [(&'static str, String); 9] {
    [
        (HDR_CONTENT_HASH, hex::encode(meta.content_hash)),
        (HDR_SIZE_BYTES, meta.size_bytes.to_string()),
        (HDR_SCHEMA_VERSION, meta.schema_version.to_string()),
        (HDR_RECIPE_NAMESPACE, meta.recipe_namespace.clone()),
        (HDR_OUTPUT_INDEX, meta.output_index.to_string()),
        (HDR_OUTPUT_PATH, meta.output_path.clone()),
        (HDR_MODE, meta.mode.to_string()),
        (HDR_KIND, meta.kind.clone().unwrap_or_default()),
        (HDR_SYMLINK_TARGET, meta.target.clone().unwrap_or_default()),
    ]
}

/// `Authorization` + the meta headers on a request builder. Used by `put`.
fn put_headers(req: ureq::Request, auth: &str, meta: &ArtifactMeta) -> ureq::Request {
    meta_to_headers(meta)
        .iter()
        .fold(req.set("Authorization", auth), |r, (n, v)| r.set(n, v))
}

/// Reconstruct an `ArtifactMeta` from `X-Cook-*` headers (`get` looks one up
/// by name).
///
/// `content_hash` is REQUIRED (CS-0058 §3.2.4); missing or malformed →
/// `Other`. All other fields default gracefully: `mode` →
/// `ArtifactMeta::default_mode()` (0o644), `kind`/`target` → `None`, numeric
/// fields → 0, string fields → "". Fields not transmitted as headers
/// (`command_hash`, `env_contribution`, `seal_contribution`, `tags`,
/// `consulted_env_keys`) use zero / empty-set defaults — they are diagnostic
/// and not needed for restore.
pub(crate) fn meta_from_headers<'a>(
    get: impl Fn(&str) -> Option<&'a str>,
) -> BackendResult<ArtifactMeta> {
    let h = get(HDR_CONTENT_HASH).ok_or_else(|| {
        BackendError::Other(format!("malformed message: missing {HDR_CONTENT_HASH} header"))
    })?;
    let mut content_hash = [0u8; 32];
    hex::decode_to_slice(h, &mut content_hash).map_err(|e| {
        BackendError::Other(format!(
            "malformed message: {HDR_CONTENT_HASH} not 64-char hex: {e}"
        ))
    })?;
    let non_empty = |n: &str| get(n).filter(|s| !s.is_empty()).map(str::to_string);
    let text = |n: &str| get(n).unwrap_or("").to_string();

    Ok(ArtifactMeta {
        recipe_namespace: text(HDR_RECIPE_NAMESPACE),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: get(HDR_SCHEMA_VERSION).and_then(|s| s.parse().ok()).unwrap_or(0),
        size_bytes: get(HDR_SIZE_BYTES).and_then(|s| s.parse().ok()).unwrap_or(0),
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: get(HDR_OUTPUT_INDEX).and_then(|s| s.parse().ok()).unwrap_or(0),
        output_path: text(HDR_OUTPUT_PATH),
        content_hash,
        kind: non_empty(HDR_KIND),
        mode: get(HDR_MODE)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(ArtifactMeta::default_mode),
        target: non_empty(HDR_SYMLINK_TARGET),
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
                    let meta = meta_from_headers(|n| response.header(n))?;
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
                if capped.done && capped.cap.exceeded() {
                    Err(BackendError::Other(capped.cap.message()))
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

    /// The manifest is serialized once and PUT from memory, so a transient
    /// failure is retried like any other idempotent call.
    fn put_manifest(&self, key: &CloudKey, manifest: &DeterminantManifest) -> BackendResult<()> {
        let body = serde_json::to_vec(manifest)
            .map_err(|e| BackendError::Other(format!("serialize manifest: {e}")))?;
        let url = self.manifest_url(key);
        let auth = self.auth_header();
        retry_loop(&self.config, || {
            self.client
                .put(&url)
                .set("Authorization", &auth)
                .set("Content-Type", "application/json")
                .send_bytes(&body)
                .map(|_| ())
                .map_err(|e| map_ureq_error(e, "put_manifest"))
        })
    }

    /// 404 is the absence case, and what a server without the route answers.
    /// A body that does not decode is absence too, never an error.
    fn get_manifest(&self, key: &CloudKey) -> BackendResult<Option<DeterminantManifest>> {
        let url = self.manifest_url(key);
        let auth = self.auth_header();
        retry_loop(&self.config, || {
            match self.client.get(&url).set("Authorization", &auth).call() {
                Ok(response) => match decode_manifest(key, response.into_reader()) {
                    Ok(m) => Ok(Some(m)),
                    Err(ManifestRefusal::Io(why)) => {
                        Err(BackendError::Transient(format!("get_manifest: {why}")))
                    }
                    Err(why) => {
                        tracing::warn!("cloud manifest for {}: {why}; treating as absent", hex::encode(key));
                        Ok(None)
                    }
                },
                Err(ureq::Error::Status(404, _)) => Ok(None),
                Err(e) => Err(map_ureq_error(e, "get_manifest")),
            }
        })
    }
}

#[cfg(test)]
#[path = "tests/cloud_backend_tests.rs"]
mod tests;
