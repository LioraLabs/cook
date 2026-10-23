//! The slice of HTTP/1.1 `CacheServer` needs, on a plain `TcpStream`: read one
//! request head, frame its body, write one `Connection: close` response, and
//! close without resetting a peer that is still sending.

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

/// A request head larger than this is refused (431).
const MAX_HEAD: usize = 64 * 1024;
const MAX_HEADERS: usize = 64;
/// After the response, read and discard what the peer is still sending, up to
/// this much and this long: closing a socket with unread input resets it, and
/// the reset can destroy the response before the client reads it.
const LINGER: Duration = Duration::from_secs(5);
const LINGER_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) struct Head {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    minor: u8,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn count(&self, name: &str) -> usize {
        self.headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)).count()
    }
}

/// Why no request could be read: say why (status, text), or just hang up.
pub(crate) enum HeadError {
    Hangup,
    Refuse(u16, &'static str),
}

/// Read up to the blank line. Bytes read past it are the start of the body.
pub(crate) fn read_head(stream: &mut impl Read) -> Result<(Head, Vec<u8>), HeadError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let mut slots = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut req = httparse::Request::new(&mut slots);
        match req.parse(&buf) {
            Ok(httparse::Status::Complete(n)) => {
                let headers = req
                    .headers
                    .iter()
                    .map(|h| Some((h.name.to_string(), std::str::from_utf8(h.value).ok()?.trim().to_string())))
                    .collect::<Option<Vec<_>>>()
                    .ok_or(HeadError::Refuse(400, "header value is not UTF-8"))?;
                let head = Head {
                    method: req.method.unwrap_or("").to_string(),
                    target: req.path.unwrap_or("").to_string(),
                    headers,
                    minor: req.version.unwrap_or(1),
                };
                return Ok((head, buf.split_off(n)));
            }
            Ok(httparse::Status::Partial) => {}
            Err(httparse::Error::TooManyHeaders) => return Err(HeadError::Refuse(431, "too many header fields")),
            Err(_) => return Err(HeadError::Refuse(400, "malformed request")),
        }
        if buf.len() >= MAX_HEAD {
            return Err(HeadError::Refuse(431, "request head too large"));
        }
        match stream.read(&mut chunk) {
            Ok(0) if buf.is_empty() => return Err(HeadError::Hangup),
            Ok(0) => return Err(HeadError::Refuse(400, "request head cut short")),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return Err(HeadError::Hangup),
        }
    }
}

/// The body length the head declares: `Some(n)` for Content-Length, `None` for
/// chunked or no body. Ambiguous framing (both headers, a repeated one, any
/// other transfer coding, a Content-Length that is not plain digits) is
/// refused: a proxy in front may read it differently. The one place framing
/// is decided.
pub(crate) fn declared_length(head: &Head) -> Result<Option<u64>, &'static str> {
    let (cl, te) = (head.count("Content-Length"), head.count("Transfer-Encoding"));
    if cl > 1 || te > 1 || (cl == 1 && te == 1) {
        return Err("ambiguous request framing");
    }
    if let Some(coding) = head.header("Transfer-Encoding") {
        if head.minor == 0 || !coding.eq_ignore_ascii_case("chunked") {
            return Err("unsupported transfer coding");
        }
        return Ok(None);
    }
    let Some(len) = head.header("Content-Length") else {
        return Ok(None);
    };
    if len.is_empty() || !len.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Content-Length is not a number");
    }
    len.parse().map(Some).map_err(|_| "Content-Length is too large")
}

/// The request's body as the head frames it (see `declared_length`).
pub(crate) fn body_reader<'a>(
    head: &Head,
    leftover: Vec<u8>,
    stream: impl Read + 'a,
) -> Result<Box<dyn Read + 'a>, &'static str> {
    let declared = declared_length(head)?;
    let source = io::Cursor::new(leftover).chain(stream);
    Ok(match declared {
        Some(left) => Box::new(Exact { inner: source, left }),
        None if head.header("Transfer-Encoding").is_some() => {
            Box::new(Chunked(chunked_transfer::Decoder::new(io::BufReader::new(source))))
        }
        None => Box::new(io::empty()),
    })
}

/// Exactly `left` bytes, or an error: a short body must not look complete.
struct Exact<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Exact<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let max = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..max])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// `chunked_transfer::Decoder` reports a stream cut inside a chunk as a clean
/// end; here that is an error.
struct Chunked<R: Read>(chunked_transfer::Decoder<R>);

impl<R: Read> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.0.read(buf)?;
        if n == 0 && !buf.is_empty() && self.0.remaining_chunks_size().is_some() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(n)
    }
}

pub(crate) enum Body {
    Bytes(Vec<u8>),
    /// A file and the length to send (read once, from its metadata).
    File(File, u64),
}

pub(crate) struct Reply {
    pub status: u16,
    headers: Vec<(String, String)>,
    pub body: Body,
}

impl Reply {
    pub fn new(status: u16, body: Body) -> Self {
        Self { status, headers: Vec::new(), body }
    }

    /// Add a header; a control byte in the value would split the response.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, ()> {
        if value.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return Err(());
        }
        self.headers.push((name.to_string(), value.to_string()));
        Ok(self)
    }

    pub fn len(&self) -> u64 {
        match &self.body {
            Body::Bytes(b) => b.len() as u64,
            Body::File(_, n) => *n,
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Internal Server Error",
    }
}

pub(crate) fn write_reply(out: &mut impl Write, reply: Reply) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reason(reply.status),
        reply.len()
    );
    for (n, v) in &reply.headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())?;
    match reply.body {
        Body::Bytes(b) => out.write_all(&b)?,
        Body::File(file, len) => {
            io::copy(&mut file.take(len), out)?;
        }
    }
    out.flush()
}

/// Close politely: send FIN, then discard what the peer still sends (bounded
/// in bytes, in total time, and by the socket's own read timeout).
pub(crate) fn close(stream: &mut TcpStream, io_timeout: Duration) {
    if stream.shutdown(Shutdown::Write).is_err() {
        return;
    }
    let (deadline, mut seen) = (Instant::now() + LINGER, 0u64);
    let mut buf = [0u8; 16 * 1024];
    while seen < LINGER_BYTES {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let per_read = io_timeout.min(left);
        if stream.set_read_timeout(Some(per_read.max(Duration::from_millis(1)))).is_err() {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => seen += n as u64,
        }
    }
}
