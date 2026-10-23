//! `CacheServer` — the v1 artifact wire protocol (see `cloud_backend`) served
//! over a `LocalBackend`. One store layout, one content-hash check: storage is
//! `LocalBackend`, the header codec is the one `CloudBackend` uses, and bodies
//! are streamed in both directions, never buffered.
//!
//! The HTTP framing is `http1`; this is routing, auth and storage.
//!
//! GET serves the stored bytes as-is; the client verifies them against
//! `X-Cook-Content-Hash`, as it does against any v1 server.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::backend::{
    ArtifactMeta, CacheBackend, CloudKey, LocalBackend, PutError,
};
use crate::cap::CapCounter;
use crate::cloud_backend::{meta_from_headers, meta_to_headers};
use crate::http1::{self, Body, HeadError, Reply};

/// A batch_query body larger than this is refused (413).
const MAX_QUERY_BODY: u64 = 16 * 1024 * 1024;
/// Socket read/write timeout: a stalled peer frees its thread after this.
const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// Connections handled at once (one thread each); past it a peer gets 503.
pub const MAX_CONNECTIONS: usize = 256;

pub struct CacheServer {
    listener: TcpListener,
    io_timeout: Duration,
    stopping: AtomicBool,
    backend: LocalBackend,
    token: String,
    max_connections: usize,
    live: AtomicUsize,
    /// Spawns still to fail on purpose, so a test can reach the spawn-error arm.
    #[cfg(test)]
    fail_spawns: AtomicUsize,
}

/// One admitted connection; frees its slot when the handler ends (or never starts).
struct Slot<'a>(&'a AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One parsed request: the head, the socket it answers on, and its body,
/// which is framed (and `100 Continue` sent) only when a route asks for it.
struct Request<'a> {
    head: http1::Head,
    leftover: Vec<u8>,
    stream: &'a TcpStream,
}

impl Request<'_> {
    fn path(&self) -> &str {
        self.head.target.split('?').next().unwrap_or("")
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.head.header(name)
    }

    fn declared_length(&self) -> Result<Option<u64>, Reply> {
        http1::declared_length(&self.head).map_err(|why| text(400, why))
    }

    fn body(&mut self) -> Result<Box<dyn Read + '_>, Reply> {
        let body = http1::body_reader(&self.head, std::mem::take(&mut self.leftover), self.stream)
            .map_err(|why| text(400, why))?;
        if self.header("Expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue")) {
            // A peer that cannot be told to go ahead fails its own read.
            let _ = (&*self.stream).write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        }
        Ok(body)
    }
}

impl CacheServer {
    pub fn bind(
        addr: impl ToSocketAddrs,
        backend: LocalBackend,
        token: String,
    ) -> io::Result<Self> {
        Self::bind_with_io_timeout(addr, backend, token, IO_TIMEOUT)
    }

    pub fn bind_with_io_timeout(
        addr: impl ToSocketAddrs,
        backend: LocalBackend,
        token: String,
        io_timeout: Duration,
    ) -> io::Result<Self> {
        if token.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cache server token must not be empty",
            ));
        }
        Ok(Self {
            listener: TcpListener::bind(addr)?,
            io_timeout,
            stopping: AtomicBool::new(false),
            backend,
            token,
            max_connections: MAX_CONNECTIONS,
            live: AtomicUsize::new(0),
            #[cfg(test)]
            fail_spawns: AtomicUsize::new(0),
        })
    }

    /// Handle at most `n` connections at once.
    pub fn with_max_connections(mut self, n: std::num::NonZeroUsize) -> Self {
        self.max_connections = n.get();
        self
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("bound to a TCP address")
    }

    /// Serve until `shutdown`; in-flight requests finish before this returns.
    /// Only the shutdown flag ends the accept loop; an accept error is logged
    /// and retried. Past `max_connections`, or when no thread can be made, a
    /// connection is refused and the loop goes on.
    pub fn run(&self) {
        let mut refusing = false;
        let mut spawn_failing = false;
        std::thread::scope(|scope| {
            loop {
                let accepted = self.listener.accept();
                if self.stopping.load(Ordering::SeqCst) {
                    break;
                }
                match accepted {
                    Ok((stream, peer)) => {
                        if self.live.load(Ordering::SeqCst) >= self.max_connections {
                            if !refusing {
                                eprintln!(
                                    "cook cache serve: {} connections open; refusing new ones with 503",
                                    self.max_connections
                                );
                                refusing = true;
                            }
                            refuse_busy(&stream);
                            continue;
                        }
                        refusing = false;
                        self.live.fetch_add(1, Ordering::SeqCst);
                        let slot = Slot(&self.live);
                        let spawned = if self.inject_spawn_failure() {
                            Err(io::Error::other("injected spawn failure"))
                        } else {
                            std::thread::Builder::new().spawn_scoped(scope, move || {
                                let _slot = slot;
                                self.handle(stream);
                            })
                        };
                        match spawned {
                            Ok(_) => spawn_failing = false,
                            Err(e) => {
                                // Once per burst: a peer can't flood the log.
                                if !spawn_failing {
                                    eprintln!("cook cache serve: dropped connection from {peer}: cannot start a thread: {e}");
                                }
                                spawn_failing = true;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("cook cache serve: accept failed: {e}");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        });
    }

    #[cfg(test)]
    fn inject_spawn_failure(&self) -> bool {
        self.fail_spawns
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }

    #[cfg(not(test))]
    fn inject_spawn_failure(&self) -> bool {
        false
    }

    /// Make `run` return: set the flag, then connect to ourselves so the
    /// blocked `accept` wakes and sees it.
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let mut addr = self.local_addr();
        if addr.ip().is_unspecified() {
            addr.set_ip(match addr.ip() {
                IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
                IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
            });
        }
        let _ = TcpStream::connect_timeout(&addr, Duration::from_secs(2));
    }

    fn handle(&self, mut stream: TcpStream) {
        let start = Instant::now();
        let timeouts = stream
            .set_nonblocking(false)
            .and_then(|()| stream.set_read_timeout(Some(self.io_timeout)))
            .and_then(|()| stream.set_write_timeout(Some(self.io_timeout)));
        if timeouts.is_err() {
            return;
        }
        let (method, path, reply) = match http1::read_head(&mut &stream) {
            Err(HeadError::Hangup) => return,
            Err(HeadError::Refuse(status, why)) => ("-".into(), "-".into(), text(status, why)),
            Ok((head, leftover)) => {
                let method = head.method.clone();
                let mut request = Request { head, leftover, stream: &stream };
                let path = request.path().to_string();
                let reply = self.route(&mut request, &method, &path);
                (method, path, reply)
            }
        };
        let (status, bytes) = (reply.status, reply.len());
        if let Err(e) = http1::write_reply(&mut &stream, reply) {
            eprintln!("cook cache serve: {method} {path}: respond failed: {e}");
        }
        http1::close(&mut stream, self.io_timeout);
        eprintln!("{method} {path} {status} {bytes} {}ms", start.elapsed().as_millis());
    }

    fn route(&self, request: &mut Request, method: &str, path: &str) -> Reply {
        if !self.authorized(request) {
            return text(401, "missing or wrong bearer token");
        }
        match path {
            "/v1/health" => {
                if method != "GET" {
                    return text(405, "use GET");
                }
                match self.backend.health() {
                    Ok(()) => text(200, "ok"),
                    Err(e) => text(500, &e.to_string()),
                }
            }
            "/v1/artifacts/batch_query" => {
                if method != "POST" {
                    return text(405, "use POST");
                }
                self.batch_query(request)
            }
            _ => {
                let Some(hex_key) = path.strip_prefix("/v1/artifacts/") else {
                    return text(404, "unknown path");
                };
                let Some(key) = parse_key(hex_key) else {
                    return text(400, "key must be 64 lowercase hex characters");
                };
                match method {
                    "GET" => self.get(&key),
                    "PUT" => self.put(request, &key),
                    "DELETE" => self.delete(&key),
                    _ => text(405, "use GET, PUT or DELETE"),
                }
            }
        }
    }

    /// Exactly `Bearer <token>`, compared without early exit.
    fn authorized(&self, request: &Request) -> bool {
        let want = format!("Bearer {}", self.token);
        let got = request.header("Authorization").unwrap_or("");
        let (want, got) = (want.as_bytes(), got.as_bytes());
        let mut diff = want.len() ^ got.len();
        for (i, w) in want.iter().enumerate() {
            diff |= (w ^ got.get(i).copied().unwrap_or(0)) as usize;
        }
        diff == 0
    }

    fn batch_query(&self, request: &mut Request) -> Reply {
        #[derive(Deserialize)]
        struct Query {
            keys: Vec<String>,
        }
        let mut body = Vec::new();
        let mut reader = match request.body() {
            Ok(r) => r,
            Err(reply) => return reply,
        };
        if let Err(e) = reader.by_ref().take(MAX_QUERY_BODY + 1).read_to_end(&mut body) {
            return text(400, &format!("read body: {e}"));
        }
        if body.len() as u64 > MAX_QUERY_BODY {
            return text(413, "batch_query body too large");
        }
        let query: Query = match serde_json::from_slice(&body) {
            Ok(q) => q,
            Err(e) => return text(400, &format!("malformed batch_query body: {e}")),
        };
        let mut keys = Vec::with_capacity(query.keys.len());
        for k in &query.keys {
            match parse_key(k) {
                Some(key) => keys.push(key),
                None => return text(400, &format!("key {k:?} is not 64 lowercase hex characters")),
            }
        }
        match self.backend.batch_query(&keys) {
            Ok(present) => {
                let present: Vec<String> = present.iter().map(hex::encode).collect();
                let body = serde_json::json!({ "present": present }).to_string();
                text(200, &body)
                    .header("Content-Type", "application/json")
                    .unwrap_or_else(|()| text(500, "content type"))
            }
            Err(e) => text(500, &e.to_string()),
        }
    }

    fn get(&self, key: &CloudKey) -> Reply {
        let (file, meta) = match self.backend.open_raw(key) {
            Ok(Some(found)) => found,
            Ok(None) => return text(404, "not found"),
            Err(e) => return text(500, &e.to_string()),
        };
        // The file's own length, never meta.size_bytes: the client must see
        // exactly what is on disk, so a mismatch fails its hash check.
        let len = match file.metadata() {
            Ok(m) => m.len(),
            Err(e) => return text(500, &format!("stat blob: {e}")),
        };
        let mut reply = Reply::new(200, Body::File(file, len));
        for (name, value) in meta_to_headers(&meta) {
            reply = match reply.header(name, &value) {
                Ok(r) => r,
                Err(()) => return text(500, &format!("{name} value is not representable in a header")),
            };
        }
        reply
    }

    fn put(&self, request: &mut Request, key: &CloudKey) -> Reply {
        let mut meta: ArtifactMeta = match meta_from_headers(|n| request.header(n)) {
            Ok(m) => m,
            Err(e) => return text(400, &e.to_string()),
        };
        // Refused before `100 Continue`, so the peer need not send the body.
        let cap = self.backend.config().max_artifact_bytes;
        match request.declared_length() {
            Err(reply) => return reply,
            Ok(Some(n)) => {
                if let Err(msg) = CapCounter::new(cap).add(n) {
                    return text(413, &msg);
                }
            }
            Ok(None) => {}
        }
        let mut body = match request.body() {
            Ok(b) => b,
            Err(reply) => return reply,
        };
        let staged = match self.backend.stage(key, &mut body, &meta) {
            Ok(s) => s,
            Err(e) => return put_failure(key, e),
        };
        match self.backend.commit(key, staged, &mut meta) {
            Ok(()) => text(200, "stored"),
            Err(e) => put_failure(key, e),
        }
    }

    fn delete(&self, key: &CloudKey) -> Reply {
        match self.backend.delete(key) {
            Ok(()) => text(204, ""),
            Err(e) => text(500, &e.to_string()),
        }
    }
}

/// Best effort and never blocking: the accept loop must not wait on a peer,
/// so a full socket buffer (WouldBlock) just drops the reply.
fn refuse_busy(stream: &TcpStream) {
    if stream.set_nonblocking(true).is_ok() {
        let _ = http1::write_reply(&mut &*stream, text(503, "too many open connections"));
    }
}

/// The caller's faults are 4xx (409 key conflict, 413 over the cap, else 400); a store-side failure is 5xx (the client
/// retries it) and is logged here, since the client only sees the status.
fn put_failure(key: &CloudKey, e: PutError) -> Reply {
    match e {
        PutError::Source(msg) | PutError::Rejected(msg) => text(400, &msg),
        PutError::Conflict(msg) => text(409, &msg),
        PutError::TooLarge(msg) => text(413, &msg),
        PutError::Store { kind, msg } => {
            eprintln!("cook cache serve: PUT {}: store error: {msg}", hex::encode(key));
            text(if kind == io::ErrorKind::StorageFull { 507 } else { 500 }, &msg)
        }
    }
}

/// Exactly 64 lowercase hex characters.
fn parse_key(s: &str) -> Option<CloudKey> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut key = [0u8; 32];
    hex::decode_to_slice(s, &mut key).ok()?;
    Some(key)
}

fn text(status: u16, body: &str) -> Reply {
    Reply::new(status, Body::Bytes(body.as_bytes().to_vec()))
}

#[cfg(test)]
#[path = "tests/serve_tests.rs"]
mod tests;
