//! Just enough HTTP for the admin page and the test backend: read one
//! request head, answer with a short plain-text body, close.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::Pool;

/// Request heads bigger than this are cut off: an endless header line from a
/// hostile client can't grow our memory.
const MAX_HEAD: u64 = 8 * 1024;
/// A client that connects and says nothing must not pin a thread forever.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The test backend: every request gets `hello from <its own address>`.
pub fn serve_hello(listener: TcpListener) {
    let body = match listener.local_addr() {
        Ok(addr) => format!("hello from {addr}\n"),
        Err(_) => "hello\n".to_string(),
    };
    serve_text(listener, "backend", move || body.clone());
}

/// The admin endpoint: every request gets the current stats table.
pub fn serve_admin(listener: TcpListener, pool: Arc<Pool>) {
    serve_text(listener, "admin", move || pool.stats_table());
}

/// Shared accept loop for both servers above. The body is a closure, so
/// one loop serves a fixed string or a freshly computed table alike.
/// `Arc<F>` lets every connection thread call the same closure.
fn serve_text<F>(listener: TcpListener, label: &'static str, body: F)
where
    F: Fn() -> String + Send + Sync + 'static,
{
    let body = Arc::new(body);
    loop {
        let (stream, peer) = match listener.accept() {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let body = Arc::clone(&body);
        thread::spawn(move || match read_request_line(&stream) {
            Ok(Some(request)) => {
                eprintln!("[{peer}] {label}: {request}");
                if let Err(e) = respond(&stream, &body()) {
                    eprintln!("[{peer}] write error: {e}");
                }
            }
            // Connected and left without a word: that's a health probe.
            Ok(None) => {}
            Err(e) => eprintln!("[{peer}] bad request: {e}"),
        });
    }
}

/// Reads one request head and returns its first line (`GET / HTTP/1.1`), or
/// `None` if the peer closed without sending anything.
fn read_request_line(stream: &TcpStream) -> io::Result<Option<String>> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    // `take` caps the total bytes this reader will ever pull from the socket.
    let mut reader = BufReader::new(stream.take(MAX_HEAD));
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Drain the headers up to the blank line even though we ignore them.
    // Closing a socket that still has unread input makes the kernel send a
    // reset (RST) instead of a normal close, and the client may lose our reply.
    let mut header = String::new();
    while reader.read_line(&mut header)? > 0 && !header.trim_end().is_empty() {
        header.clear();
    }

    let line = request_line.trim_end();
    Ok((!line.is_empty()).then(|| line.to_string()))
}

fn respond(mut stream: &TcpStream, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
