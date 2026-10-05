//! httpd — a static file server speaking HTTP/1.1 over raw TCP.
//!
//! # HTTP/1.1 on the wire (RFC 9112)
//!
//! An HTTP message is a *head* of text lines, each ending in CRLF, then an
//! empty line, then an optional *body* of raw bytes. This is what
//! `curl 127.0.0.1:8180/files/hello.txt` actually sends:
//!
//! ```text
//! GET /files/hello.txt HTTP/1.1\r\n   request line: method SP target SP version (§3)
//! Host: 127.0.0.1:8180\r\n            header field: name ":" OWS value OWS (§5)
//! User-Agent: curl/8.7.1\r\n
//! Accept: */*\r\n
//! \r\n                                empty line: the head is over
//! ```
//!
//! and what comes back:
//!
//! ```text
//! HTTP/1.1 200 OK\r\n                 status line: version SP code SP reason (§4)
//! Content-Type: text/plain; charset=utf-8\r\n
//! Content-Length: 14\r\n              exactly 14 body bytes follow (§6.3)
//! \r\n
//! Hello, world!\n                     the body; nothing marks its end but the length
//! ```
//!
//! The parts that matter when you write one yourself:
//!
//! - **Framing.** The head ends at the first empty line. The body is
//!   framed by `Content-Length`; without it, a request has no body. Since
//!   nothing else marks where a message ends, a wrong length desyncs every
//!   later message on the connection.
//! - **Headers.** Names are case-insensitive (`host` = `Host`) and the
//!   value has optional whitespace around it. HTTP/1.1 requests must carry
//!   `Host`.
//! - **Persistence (§9.3).** An HTTP/1.1 connection stays open after a
//!   response, so the next request reuses it, until a side sends
//!   `Connection: close`. HTTP/1.0 connections close after one response.
//! - **Leniency (§2.2).** Accept a bare LF as a line ending, but reject a
//!   bare CR. Bound every size: a server that waits for "the end of the
//!   line" can be fed bytes forever.
//!
//! # Layout
//!
//! - [`http`]: parse a `Request` from any `BufRead`, write a `Response`
//!   to any `Write`. No sockets.
//! - [`files`]: URL path → file under the root, MIME types, listings.
//! - [`pool`]: the fixed-size thread pool.
//! - this file: the accept loop and the per-connection keep-alive loop.

pub mod files;
pub mod http;
pub mod pool;

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use http::{ParseError, Request, Response};
use pool::ThreadPool;

#[derive(Debug, Clone)]
pub struct Config {
    pub root: PathBuf,
    pub workers: usize,
    /// How long a connection may sit idle before we close it. An idle
    /// keep-alive connection occupies a pool thread, so keep this short.
    pub idle_timeout: Duration,
}

impl Config {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        // 8 workers: a browser opens up to 6 connections per host.
        Config {
            root: root.into(),
            workers: 8,
            idle_timeout: Duration::from_secs(5),
        }
    }
}

/// The accept loop: every connection becomes one job on the pool. Only
/// returns (with an error) if the root directory can't be opened.
pub fn serve(listener: TcpListener, config: Config) -> io::Result<()> {
    // Absolute, with symlinks resolved, for the escape check in `respond`.
    // `Arc` lets every job share one copy.
    let root = Arc::new(config.root.canonicalize()?);
    let idle = config.idle_timeout;
    let pool = ThreadPool::new(config.workers);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let root = Arc::clone(&root);
                pool.execute(move || {
                    if let Err(e) = handle_connection(stream, &root, idle) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
    Ok(())
}

/// Answers requests on one connection until the client closes it, asks to
/// close, goes idle, or sends something we can't parse.
fn handle_connection(stream: TcpStream, root: &Path, idle: Duration) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    // A read that waits longer than `idle` returns an error. That ends the
    // loop below and frees this worker for someone else.
    stream.set_read_timeout(Some(idle))?;
    stream.set_write_timeout(Some(idle))?;
    // `&TcpStream` implements both Read and Write, so a reader and a
    // writer can borrow the same socket. No `try_clone` needed.
    let mut reader = BufReader::new(&stream);
    loop {
        let start = Instant::now();
        match http::parse_request(&mut reader) {
            // Clean close between requests, idle timeout, or reset.
            Ok(None) | Err(ParseError::Io(_)) => return Ok(()),
            Ok(Some(req)) => {
                let keep_alive = req.keep_alive();
                let mut resp = respond(&req, root);
                if !keep_alive {
                    resp = resp.with_header("Connection", "close");
                }
                let head_only = req.method == "HEAD";
                send(&stream, &resp, head_only)?;
                log(peer, &req.method, &req.target, &resp, head_only, start);
                if !keep_alive {
                    return Ok(());
                }
            }
            Err(e) => {
                let body = format!("{} {}: {e}\n", e.status(), http::reason(e.status()));
                let resp = Response::new(e.status(), "text/plain; charset=utf-8", body)
                    .with_header("Connection", "close");
                send(&stream, &resp, false)?;
                log(peer, "-", "-", &resp, false, start);
                linger_close(&stream);
                return Ok(());
            }
        }
    }
}

/// Decides the response to one request. It reads the filesystem but never
/// touches the socket.
pub fn respond(req: &Request, root: &Path) -> Response {
    if req.method != "GET" && req.method != "HEAD" {
        return Response::plain(405).with_header("Allow", "GET, HEAD");
    }
    let path = match files::resolve(root, &req.target) {
        Ok(path) => path,
        Err(status) => return Response::plain(status),
    };
    // `resolve` only looks at the text. A symlink inside the root could
    // still point outside it, so follow links and check again.
    let path = match path.canonicalize() {
        Ok(real) if real.starts_with(root) => real,
        Ok(_) => return Response::plain(403),
        Err(_) => return Response::plain(404),
    };
    if !path.is_dir() {
        return files::serve_file(&path);
    }
    let url_path = req.path();
    if !url_path.ends_with('/') {
        // `/files` → `/files/`, so relative links on the page resolve
        // inside the directory rather than next to it. Collapse leading
        // slashes: `//evil.com/` would send the browser to another host.
        let location = format!("/{}/", url_path.trim_start_matches('/'));
        return Response::plain(301).with_header("Location", &location);
    }
    let index = path.join("index.html");
    if index.is_file() {
        files::serve_file(&index)
    } else {
        files::serve_listing(&path, url_path)
    }
}

fn send(stream: &TcpStream, resp: &Response, head_only: bool) -> io::Result<()> {
    // Collect the whole response and send it in as few writes as possible.
    // Many tiny writes cost a syscall each and can trip Nagle's algorithm
    // into a delayed-ACK stall on keep-alive connections.
    let mut out = BufWriter::new(stream);
    resp.write_to(&mut out, head_only)?;
    out.flush()
}

/// After an error reply the client may still be sending, such as the rest
/// of an oversized header. Closing a socket with unread input makes the
/// kernel send RST, which can destroy our reply before the client reads
/// it. So send FIN first, then drain what's left for a moment ("lingering
/// close", RFC 9112 §9.6).
fn linger_close(stream: &TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = io::copy(&mut stream.take(1024 * 1024), &mut io::sink());
}

/// One access-log line per request:
/// `127.0.0.1:52000 GET /style.css 200 1234B 0.2ms`.
fn log(
    peer: SocketAddr,
    method: &str,
    target: &str,
    resp: &Response,
    head_only: bool,
    start: Instant,
) {
    let bytes = if head_only { 0 } else { resp.body.len() };
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "{peer} {method} {target} {} {bytes}B {ms:.1}ms",
        resp.status
    );
}
