//! restapi — a tiny web framework plus a todo API, over hand-written
//! HTTP/1.1.
//!
//! # A JSON API on the wire (RFC 9112)
//!
//! `curl -s -X POST -d '{"title":"write a router"}' 127.0.0.1:8181/todos`
//! sends:
//!
//! ```text
//! POST /todos HTTP/1.1\r\n        method + path: *what* to do to *which* resource
//! Host: 127.0.0.1:8181\r\n
//! User-Agent: curl/8.7.1\r\n
//! Accept: */*\r\n
//! Content-Length: 26\r\n          the body is exactly 26 bytes (§6.3)
//! Content-Type: application/x-www-form-urlencoded\r\n   curl's default for -d; we ignore it
//! \r\n
//! {"title":"write a router"}      body: read with take(26), never past it
//! ```
//!
//! and the answer:
//!
//! ```text
//! HTTP/1.1 201 Created\r\n        201, not 200: a new resource exists
//! Content-Type: application/json\r\n
//! Location: /todos/1\r\n          where the new resource lives
//! Content-Length: 47\r\n          46 bytes of JSON + a newline
//! \r\n
//! {"id":1,"title":"write a router","done":false}\n
//! ```
//!
//! REST maps the method onto the resource:
//!
//! | request              | meaning               | success           |
//! |----------------------|-----------------------|-------------------|
//! | `GET /todos`         | list (`?done=true`)   | 200 + array       |
//! | `POST /todos`        | create                | 201 + `Location`  |
//! | `GET /todos/:id`     | show                  | 200               |
//! | `PATCH /todos/:id`   | change some fields    | 200               |
//! | `DELETE /todos/:id`  | remove                | 204, empty body   |
//!
//! Errors are JSON too: 400 for bad JSON, 404 for an unknown id or path,
//! 405 (with `Allow`) for a known path with the wrong method, and 429 (with
//! `Retry-After`) once a client goes over its rate limit.
//!
//! # Layout
//!
//! ```text
//! socket ─► http::parse_request ─► Router::handle ─► Logger ─► RateLimiter ─► route ─► handler
//! socket ◄─ Response::write_to ◄──────────────── Response ◄───────────────────────────┘
//! ```
//!
//! Only this file touches sockets. [`http`], [`router`], [`middleware`],
//! and [`todos`] are plain functions and traits that tests call directly.

pub mod http;
pub mod middleware;
pub mod router;
pub mod todos;

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use http::{ParseError, Response};
use middleware::{Logger, RateLimiter};
use router::Router;
use todos::SharedStore;

/// Rate limit per client IP: bursts of 10, then 5 requests per second.
pub const RATE_BURST: u32 = 10;
pub const RATE_PER_SEC: u32 = 5;
pub const WORKERS: usize = 8;
/// An idle keep-alive connection holds a worker, so don't let it idle long.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// The todo API with logging and rate limiting. The logger is the
/// outermost layer, so it sees (and logs) the 429s too.
pub fn app(store: SharedStore) -> Router {
    todos::routes(store)
        .wrap(Logger)
        .wrap(RateLimiter::new(RATE_BURST, RATE_PER_SEC))
}

/// Runs `workers` threads that each loop on `accept()`. This is a thread
/// pool without a job queue: the kernel hands each new connection to one of
/// the threads blocked in `accept`. (httpd uses a channel instead; compare.)
pub fn serve(listener: TcpListener, router: Router, workers: usize) {
    // Scoped threads may borrow `listener` and `router` from this stack
    // frame, with no `Arc`, because `scope` doesn't return until every
    // thread has finished.
    thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            if let Err(e) = handle_connection(stream, peer, &router) {
                                eprintln!("connection error: {e}");
                            }
                        }
                        Err(e) => eprintln!("accept error: {e}"),
                    }
                }
            });
        }
    });
}

/// Serves requests on one connection until it closes, idles out, or sends
/// something unparseable.
fn handle_connection(stream: TcpStream, peer: SocketAddr, router: &Router) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(IDLE_TIMEOUT))?;
    let mut reader = BufReader::new(&stream);
    loop {
        let start = Instant::now();
        let (resp, close, failed) = match http::parse_request(&mut reader, peer) {
            Ok(None) | Err(ParseError::Io(_)) => return Ok(()),
            Ok(Some(req)) => {
                let close = req.close;
                (router.handle(req), close, false)
            }
            Err(ParseError::Reject(status, why)) => {
                let resp = Response::error(status, why);
                middleware::log(peer, "-", "-", &resp, start);
                (resp, true, true)
            }
        };
        let resp = if close {
            resp.with_header("Connection", "close")
        } else {
            resp
        };
        // Buffer the response so it leaves in one write, not one per line.
        let mut out = BufWriter::new(&stream);
        resp.write_to(&mut out)?;
        out.flush()?;
        if failed {
            linger_close(&stream);
        }
        if close {
            return Ok(());
        }
    }
}

/// After rejecting a request (e.g. 413), the client may still be sending
/// the body. Closing with unread data makes the kernel send RST, which can
/// wipe out our reply before the client reads it. Send FIN, then drain
/// briefly (RFC 9112 §9.6).
fn linger_close(stream: &TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = io::copy(&mut stream.take(1024 * 1024), &mut io::sink());
}
