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
    let mut body = Vec::new();
    match std::fs::File::open(&file) {
        Ok(mut f) => {
            f.read_to_end(&mut body)?;
        }
        Err(_) => return respond(stream, 404, "text/plain", b"not found"),
    }
    let ctype = content_type(&file);
    if method == "HEAD" {
        body.clear();
    }
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
}
