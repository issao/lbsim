//! A minimal static file server, so the stand-in dashboard and the generated reports can be looked
//! at from a URL.
//!
//! Written rather than pulled in for two reasons. It keeps the container a single static binary with
//! no base image beyond a libc, which is the cheapest thing to build and the smallest thing to pull
//! on a cold start. And it is the skeleton the real Ingress service grows into: the health check, the
//! port contract and the request loop are the same either way.
//!
//! Deliberately not general purpose. No TLS, since Cloud Run terminates it. No caching, no ranges, no
//! compression. It serves a directory over HTTP/1.1 and nothing else.

pub mod export;
pub mod idle;
pub mod lease;
pub mod run;
pub mod server;
pub mod trace_wire;
pub mod wire;
pub use server::Server;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// A scratch `/tmp` directory that removes itself on drop, shared by every `#[cfg(test)]` module
/// in this crate that needs a real filesystem root (`run.rs`, `server.rs`, `export.rs`). A private
/// copy of `tests/common`'s `ScratchDir` (U110b): a crate-internal test module cannot import a
/// path under `tests/`, so the ten-odd lines live twice rather than once.
#[cfg(test)]
pub(crate) mod test_scratch {
    pub(crate) struct ScratchDir(std::path::PathBuf);

    impl std::ops::Deref for ScratchDir {
        type Target = std::path::Path;
        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    /// A fresh, created directory `/tmp/lbsim-<name>-<pid>`.
    pub(crate) fn scratch(name: &str) -> ScratchDir {
        let dir = std::env::temp_dir().join(format!("lbsim-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ScratchDir(dir)
    }
}

/// Cap on concurrent connections. A bound rather than a thread pool because the workload is a handful
/// of browser tabs, and an unbounded thread-per-connection server is a way to be taken down by a port
/// scanner.
const MAX_CONNECTIONS: usize = 64;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// A `StartRun` body is a scenario file plus overrides, a few kilobytes; this is the ceiling on any
/// request body, well above that and well below anything that could exhaust the box.
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// A static file above this size goes out with `Transfer-Encoding: chunked` instead of a
/// `Content-Length`. Cloud Run caps a response that declares its length at 32 MiB and answers the
/// client with a 500 and an empty body past it, which is how `replicas.jsonl` at 256 replicas took
/// the replay dashboard down on 2026-09-08 while every smaller document served. A chunked response
/// has no such cap. 4 MiB is far enough under the cap that no file can reach it, and above every
/// document a dashboard load fetches apart from the per-replica rows.
pub const CHUNKED_THRESHOLD_BYTES: u64 = 4 * 1024 * 1024;
const CHUNK_BYTES: usize = 64 * 1024;

pub fn serve(dir: &str, port: u16) -> Result<(), String> {
    let root = std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))?;
    let listener = TcpListener::bind(("0.0.0.0", port))
        .map_err(|e| format!("bind 0.0.0.0:{port}: {e}"))?;
    println!("serving {} on 0.0.0.0:{port}", root.display());
    serve_on(Arc::new(Server::new(root, idle::idle_threshold_ns_from_env())), listener)
}

/// The accept loop on a listener the caller bound, with the idle threshold injected through the
/// server rather than read from the environment. What tests use, on an ephemeral port, keeping
/// their own handle on the server to look at run state the wire does not expose.
pub fn serve_on(server: Arc<Server>, listener: TcpListener) -> Result<(), String> {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let connections = server.connections.load(Ordering::Relaxed);
        if connections >= MAX_CONNECTIONS {
            // Shed rather than queue. A refused connection is a clear signal; a growing thread count
            // is the failure that looks like a hang.
            server.log(format!("busy 503 connections={connections}"));
            let _ = respond(stream, 503, "text/plain", b"busy");
            continue;
        }
        server.connections.fetch_add(1, Ordering::Relaxed);
        let server = Arc::clone(&server);
        std::thread::spawn(move || {
            let _ = handle(stream, &server);
            server.connections.fetch_sub(1, Ordering::Relaxed);
        });
    }
    Ok(())
}

fn handle(mut stream: TcpStream, server: &Server) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    if line.len() > MAX_REQUEST_BYTES {
        return respond(stream, 431, "text/plain", b"request line too long");
    }
    // Headers are kept: the ingress routes need `Content-Length`, `Last-Event-ID` and `Expect`.
    // The static routes ignore them, but the connection must be read before replying either way or
    // some clients see a reset instead of the response.
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut header = String::new();
    let mut total = line.len();
    loop {
        header.clear();
        let n = reader.read_line(&mut header)?;
        total += n;
        if n == 0 || header == "\r\n" || header == "\n" || total > MAX_REQUEST_BYTES {
            break;
        }
        if let Some((k, v)) = header.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let path_only = target.split(['?', '#']).next().unwrap_or("/");

    if path_only.starts_with(server::INGRESS_PREFIX) {
        let length = headers
            .iter()
            .find(|(k, _)| k == "content-length")
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        if length > MAX_BODY_BYTES {
            return respond(stream, 413, "text/plain", b"request body too large");
        }
        // curl sends `Expect: 100-continue` for bodies above a threshold and waits for the nod
        // before sending them; without this the read below stalls until curl gives up.
        if headers.iter().any(|(k, v)| k == "expect" && v.eq_ignore_ascii_case("100-continue")) {
            stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            stream.flush()?;
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body)?;
        let req = server::Request {
            method: method.to_string(),
            path: path_only.to_string(),
            query: target.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or("")).unwrap_or("").to_string(),
            headers,
            body,
        };
        // A stream can outlive the request read timeout by hours; it paces itself.
        stream.set_read_timeout(None)?;
        return server.handle(req, stream);
    }

    if method != "GET" && method != "HEAD" {
        return respond(stream, 405, "text/plain", b"method not allowed");
    }

    if is_health_path(path_only) {
        return respond(stream, 200, "text/plain", b"ok");
    }
    if path_only == "/requests.log" {
        return respond(stream, 200, "text/plain", server.request_log().as_bytes());
    }

    let Some(file) = resolve(&server.root, path_only) else {
        return respond(stream, 404, "text/plain", b"not found");
    };
    let Ok(mut f) = std::fs::File::open(&file) else {
        return respond(stream, 404, "text/plain", b"not found");
    };
    let ctype = content_type(&file);
    if method == "HEAD" {
        return respond(stream, 200, ctype, b"");
    }
    if f.metadata()?.len() > CHUNKED_THRESHOLD_BYTES {
        return respond_chunked(stream, ctype, &mut f);
    }
    let mut body = Vec::new();
    f.read_to_end(&mut body)?;
    respond(stream, 200, ctype, &body)
}

/// Health check, answered without touching the filesystem or any run state. Cloud Run polls this, and
/// a check that did real work would mark a busy instance unhealthy and kill it mid-simulation.
///
/// Two paths, because on `*.run.app` Google's front end answers `GET /healthz` itself with a 404 and
/// never forwards it to the container, verified with curl: no `x-cloud-trace-context` on that path,
/// present on every other. `/healthz` stays for Cloud Run's own startup probe, which talks to the
/// container port directly and bypasses the front end; `/health` is what reaches us from outside.
pub fn is_health_path(path: &str) -> bool {
    path == "/healthz" || path == "/health"
}

/// Map a URL path to a file inside `root`, refusing anything that escapes it.
///
/// Rejects `..` by component rather than by string matching, because `%2e%2e` and mixed separators
/// defeat string checks. A static server is the classic place to leak a filesystem.
fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(url_path);
    let rel = decoded.trim_start_matches('/');
    let mut out = root.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            // Anything else is an attempt to leave the root.
            _ => return None,
        }
    }
    if out.is_dir() {
        out.push("index.html");
    }
    let canonical = std::fs::canonicalize(&out).ok()?;
    if !canonical.starts_with(root) {
        return None;
    }
    if canonical.is_file() {
        Some(canonical)
    } else {
        None
    }
}

pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn content_type(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("csv") => "text/csv; charset=utf-8",
        Some("txt") | Some("md") => "text/plain; charset=utf-8",
        Some("jsonl") => "text/plain; charset=utf-8",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

pub(crate) fn respond(mut stream: TcpStream, code: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// A 200 whose body is streamed from `file` as HTTP/1.1 chunks: the head, then `CHUNK_BYTES`-sized
/// chunks as `<hex length>\r\n<bytes>\r\n`, then the terminating `0\r\n\r\n`. See
/// `CHUNKED_THRESHOLD_BYTES` for why a large file is never sent with a `Content-Length`.
fn respond_chunked(stream: TcpStream, ctype: &str, file: &mut std::fs::File) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::with_capacity(CHUNK_BYTES + 64, stream);
    write!(
        out,
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nTransfer-Encoding: chunked\r\n\
         X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n"
    )?;
    let mut buf = vec![0u8; CHUNK_BYTES];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        write!(out, "{n:x}\r\n")?;
        out.write_all(&buf[..n])?;
        out.write_all(b"\r\n")?;
    }
    out.write_all(b"0\r\n\r\n")?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_health_paths_are_recognised_and_nothing_else_is() {
        assert!(is_health_path("/healthz"));
        assert!(is_health_path("/health"));
        assert!(!is_health_path("/"));
        assert!(!is_health_path("/healthz/"));
        assert!(!is_health_path("/health.html"));
    }

    #[test]
    fn traversal_cannot_escape_the_root() {
        let root = std::env::temp_dir();
        assert!(resolve(&root, "/../../etc/passwd").is_none());
        assert!(resolve(&root, "/%2e%2e/%2e%2e/etc/passwd").is_none());
    }

    #[test]
    fn jsonl_is_served_as_text() {
        // U55: fleet.jsonl was falling through to application/octet-stream.
        assert_eq!(content_type(Path::new("fleet.jsonl")), "text/plain; charset=utf-8");
    }

    /// One GET over TCP, the whole response read to EOF (the server closes every connection), the
    /// body de-chunked when the head says it is chunked. Returns the headers and the body.
    fn get_whole(addr: std::net::SocketAddr, path: &str) -> (Vec<(String, String)>, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("end of head");
        let head = std::str::from_utf8(&raw[..head_end]).unwrap();
        let mut lines = head.split("\r\n");
        assert!(lines.next().unwrap().starts_with("HTTP/1.1 200 "), "{head}");
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        let mut rest = &raw[head_end + 4..];
        if !headers.iter().any(|(k, v)| k == "transfer-encoding" && v == "chunked") {
            return (headers, rest.to_vec());
        }
        let mut body = Vec::new();
        loop {
            let eol = rest.windows(2).position(|w| w == b"\r\n").expect("chunk size line");
            let n = usize::from_str_radix(std::str::from_utf8(&rest[..eol]).unwrap(), 16).unwrap();
            rest = &rest[eol + 2..];
            if n == 0 {
                assert_eq!(rest, b"\r\n", "nothing follows the terminating chunk");
                return (headers, body);
            }
            body.extend_from_slice(&rest[..n]);
            assert_eq!(&rest[n..n + 2], b"\r\n", "every chunk ends in CRLF");
            rest = &rest[n + 2..];
        }
    }

    #[test]
    fn a_large_static_file_is_chunked_and_arrives_whole() {
        let dir = test_scratch::scratch("chunked");
        // 40 MB, above Cloud Run's 32 MiB cap on a response with a length, in a pattern that is
        // not periodic at the chunk size, so a dropped, repeated or reordered chunk changes it.
        let big: Vec<u8> = (0..40_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        std::fs::write(dir.join("replicas.jsonl"), &big).unwrap();
        std::fs::write(dir.join("small.json"), b"{}\n").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = Arc::new(Server::new(dir.to_path_buf(), 3600 * 1_000_000_000));
        std::thread::spawn(move || serve_on(server, listener));

        let (headers, body) = get_whole(addr, "/replicas.jsonl");
        assert!(headers.iter().any(|(k, v)| k == "transfer-encoding" && v == "chunked"), "{headers:?}");
        assert!(!headers.iter().any(|(k, _)| k == "content-length"), "{headers:?}");
        assert_eq!(body.len(), big.len());
        assert!(body == big, "the de-chunked body is the file");

        // Below the threshold nothing changes: a length, no chunking.
        let (headers, body) = get_whole(addr, "/small.json");
        assert!(headers.iter().any(|(k, v)| k == "content-length" && v == "3"), "{headers:?}");
        assert!(!headers.iter().any(|(k, _)| k == "transfer-encoding"), "{headers:?}");
        assert_eq!(body, b"{}\n");
    }
}
