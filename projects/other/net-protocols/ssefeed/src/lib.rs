//! ssefeed — a Server-Sent Events broadcast server.
//!
//! Server-Sent Events (WHATWG HTML Standard §9.2) push messages from a server
//! to a browser over one ordinary HTTP response that never ends. The browser
//! side is `new EventSource("/events")`. The wire format is plain text, one
//! `field: value` per line, and a blank line ends each event:
//!
//! ```text
//! id: 7               <- the browser remembers it and sends it back as
//!                        `Last-Event-ID: 7` when it reconnects
//! event: message      <- the event type: "message" fires `onmessage`,
//!                        other names need `addEventListener(name, …)`
//! data: first line    <- several data: lines are joined with "\n"
//! data: second line
//!                     <- blank line: dispatch the event
//! : ping              <- a line starting with ':' is a comment; we send
//!                        one as a heartbeat so idle connections stay open
//! ```
//!
//! A response that never ends can't have a `Content-Length`, so the body
//! uses chunked transfer coding (RFC 9112 §7.1): each chunk is its size in
//! hex, CRLF, that many bytes, CRLF. A zero-size chunk ends the body. We send
//! exactly one chunk per event. An annotated exchange (`\n` is a newline
//! inside a chunk; every other line ends in CRLF):
//!
//! ```text
//! C: GET /events HTTP/1.1
//! C: Host: 127.0.0.1:8182
//! C: Last-Event-ID: 2                     <- "I've seen up to id 2"
//! C:
//! S: HTTP/1.1 200 OK
//! S: Content-Type: text/event-stream
//! S: Cache-Control: no-cache
//! S: Transfer-Encoding: chunked
//! S:
//! S: 1f                                   <- 0x1f = 31 bytes follow
//! S: id: 3\nevent: message\ndata: hi\n\n  <- event 3, replayed from history
//! S: 8
//! S: : ping\n\n                           <- 15 s of silence: heartbeat
//! ```
//!
//! Endpoints:
//!
//! - `GET /` serves the demo page.
//! - `GET /events` subscribes. `Last-Event-ID: N` first replays events after N.
//! - `POST /publish` broadcasts the body. `?event=NAME` sets the event type.
//!
//! Inside, a [`Hub`] behind a `Mutex` keeps the last [`HISTORY`] events in a
//! ring buffer, plus one `mpsc::Sender` per subscriber. Each subscriber's
//! connection thread waits on its `Receiver` and writes what arrives.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// The demo page, compiled into the binary so it runs from any directory.
pub const INDEX_HTML: &str = include_str!("../static/index.html");
/// How many past events we keep for clients that reconnect.
pub const HISTORY: usize = 100;
/// Most bytes we accept for a request line plus headers.
pub const MAX_HEAD: usize = 8 * 1024;
/// Most bytes we accept for one published message.
pub const MAX_BODY: usize = 64 * 1024;
/// The production heartbeat interval. `serve` takes it as a parameter so
/// tests can pass a few milliseconds instead.
pub const HEARTBEAT: Duration = Duration::from_secs(15);
/// How long a client may take to send its request, and how long a write to a
/// subscriber that stopped reading may block before we give up on it.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

// ------------------------------------------------------------------ events

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: u64,
    pub name: String,
    pub data: String,
}

/// Formats one event for the wire. Multi-line data becomes several `data:`
/// lines, because a raw newline inside a field would end it.
pub fn format_event(event: &Event) -> String {
    let mut out = format!("id: {}\nevent: {}\n", event.id, event.name);
    // SSE treats \r\n, \n and a lone \r all as line endings, so split on all three.
    for line in event.data.replace("\r\n", "\n").split(['\n', '\r']) {
        out.push_str("data: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out
}

/// The event type from a `/publish` query string: `""` gives "message" and
/// `"event=alert"` gives "alert". Anything else is refused. Names are limited
/// to `[A-Za-z0-9_-]` so they can never smuggle a newline into the stream.
pub fn event_name(query: &str) -> Option<&str> {
    if query.is_empty() {
        return Some("message");
    }
    let name = query.strip_prefix("event=")?;
    let valid = !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    valid.then_some(name)
}

/// A `Write` adapter that sends every `write` call as one HTTP chunk.
/// Wrap a `BufWriter` so the size line, the data and the CRLF leave in a
/// single syscall when you `flush`.
pub struct ChunkedWriter<W: Write> {
    inner: W,
}

impl<W: Write> ChunkedWriter<W> {
    pub fn new(inner: W) -> Self {
        ChunkedWriter { inner }
    }

    /// Writes the zero-size last chunk that ends the body. Taking `self` by
    /// value means nobody can write another chunk after it.
    pub fn finish(mut self) -> io::Result<W> {
        self.inner.write_all(b"0\r\n\r\n")?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for ChunkedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // A zero-size chunk would mean "end of body", so never send one here.
        if buf.is_empty() {
            return Ok(0);
        }
        write!(self.inner, "{:x}\r\n", buf.len())?;
        self.inner.write_all(buf)?;
        self.inner.write_all(b"\r\n")?;
        // Claiming the whole buffer means `write_all` makes exactly one chunk.
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

// ------------------------------------------------------------------ hub

/// Everything shared between connections. Each operation is quick, so one
/// `Mutex` around the whole hub is enough. (websock solves the same fan-out
/// with a hub thread instead; compare the two.)
#[derive(Default)]
pub struct Hub {
    last_id: u64,
    history: VecDeque<Event>,
    subscribers: Vec<Sender<Event>>,
}

pub type SharedHub = Arc<Mutex<Hub>>;

impl Hub {
    /// Stores the event in history, sends it to every subscriber, and returns its id.
    pub fn publish(&mut self, name: &str, data: &str) -> u64 {
        self.last_id += 1;
        let event = Event {
            id: self.last_id,
            name: name.to_string(),
            data: data.to_string(),
        };
        // `send` fails only when the Receiver is gone, which happens when that
        // subscriber's thread hit a write error and exited. That's our cue to
        // prune it: `retain` keeps exactly the senders that still work.
        self.subscribers.retain(|tx| tx.send(event.clone()).is_ok());
        // VecDeque as a ring buffer: push at the back, drop from the front.
        self.history.push_back(event);
        if self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.last_id
    }

    /// Registers a subscriber. Returns the events it missed (ids after
    /// `last_seen`) and a receiver for live ones. Both happen under the same
    /// lock, so no event can slip in between the replay and the live feed.
    pub fn subscribe(&mut self, last_seen: Option<u64>) -> (Vec<Event>, Receiver<Event>) {
        let missed = match last_seen {
            Some(last) => self
                .history
                .iter()
                .filter(|e| e.id > last)
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        let (tx, rx) = mpsc::channel();
        self.subscribers.push(tx);
        (missed, rx)
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }
}

// ------------------------------------------------------------------ HTTP

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    /// Header names are case-insensitive (RFC 9110 §5.1).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug)]
pub enum HeadError {
    /// The socket failed, timed out, or closed early: nobody left to answer.
    Io(io::Error),
    /// More than `MAX_HEAD` bytes without reaching the blank line.
    TooLarge,
    Malformed(&'static str),
}

impl From<io::Error> for HeadError {
    fn from(e: io::Error) -> Self {
        HeadError::Io(e)
    }
}

/// Reads the request line and headers up to the blank line (RFC 9112 §2.1).
/// It stops right there, so any body is still waiting in `reader`.
pub fn read_head(reader: &mut impl BufRead) -> Result<Request, HeadError> {
    // `take` caps how much a client can make us read before the blank line.
    let mut limited = reader.take(MAX_HEAD as u64);
    let mut lines = Vec::new();
    loop {
        let mut raw = Vec::new();
        limited.read_until(b'\n', &mut raw)?;
        if raw.last() != Some(&b'\n') {
            // No newline: either we hit the cap or the peer hung up mid-head.
            return Err(if limited.limit() == 0 {
                HeadError::TooLarge
            } else {
                HeadError::Io(io::ErrorKind::UnexpectedEof.into())
            });
        }
        let line = String::from_utf8(raw).map_err(|_| HeadError::Malformed("not UTF-8"))?;
        // Accept a bare LF as well as CRLF (RFC 9112 §2.2 allows it).
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        lines.push(line.to_string());
    }

    let mut lines = lines.into_iter();
    let request_line = lines.next().ok_or(HeadError::Malformed("empty request"))?;
    let parts: Vec<&str> = request_line.split(' ').collect();
    let [method, target, version] = parts[..] else {
        return Err(HeadError::Malformed("bad request line"));
    };
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/1.") {
        return Err(HeadError::Malformed("bad request line"));
    }
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or(HeadError::Malformed("header without a colon"))?;
        // Whitespace in a name (which also catches obsolete line folding) is a
        // request-smuggling trick, so RFC 9112 §5.1 says to reject it.
        if name.is_empty() || name.contains([' ', '\t']) {
            return Err(HeadError::Malformed("bad header name"));
        }
        headers.push((
            name.to_string(),
            value.trim_matches([' ', '\t']).to_string(),
        ));
    }
    Ok(Request {
        method: method.to_string(),
        target: target.to_string(),
        headers,
    })
}

/// Writes a complete response with a fixed-length body. Head and body go out
/// in one `write_all` so they share a TCP segment.
fn reply(out: &mut impl Write, status: &str, content_type: &str, body: &str) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    out.write_all(response.as_bytes())
}

/// `POST /publish`: validates the body and broadcasts it. Returns the status
/// line and the text to answer with.
fn publish(
    req: &Request,
    query: &str,
    body: &mut impl Read,
    hub: &SharedHub,
) -> (&'static str, String) {
    let bad = |why: &str| ("400 Bad Request", format!("{why}\n"));
    let Some(len) = req.header("content-length") else {
        return ("411 Length Required", "send a Content-Length\n".into());
    };
    let Ok(len) = len.parse::<usize>() else {
        return bad("bad Content-Length");
    };
    if len > MAX_BODY {
        return (
            "413 Content Too Large",
            format!("messages are limited to {MAX_BODY} bytes\n"),
        );
    }
    // Read the body before any other check. Closing a socket with unread
    // input makes the kernel send a reset, which can destroy our reply.
    let mut data = vec![0; len];
    if body.read_exact(&mut data).is_err() {
        return bad("body shorter than Content-Length");
    }
    let Some(name) = event_name(query) else {
        return bad("use ?event=NAME with NAME made of [A-Za-z0-9_-]");
    };
    let Ok(data) = String::from_utf8(data) else {
        return bad("body must be UTF-8");
    };
    if data.is_empty() {
        return bad("empty message");
    }
    let id = hub.lock().expect("hub lock").publish(name, &data);
    ("200 OK", format!("published event {id}\n"))
}

/// `GET /events`: replays missed events, then streams live ones (with a
/// heartbeat whenever it's quiet) until a write fails because the client left.
fn stream_events(
    socket: TcpStream,
    hub: &SharedHub,
    last_seen: Option<u64>,
    heartbeat: Duration,
) -> io::Result<()> {
    let (missed, events) = hub.lock().expect("hub lock").subscribe(last_seen);
    let mut out = BufWriter::new(socket);
    out.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n",
    )?;
    let mut body = ChunkedWriter::new(out);
    for event in &missed {
        body.write_all(format_event(event).as_bytes())?;
    }
    // Without a flush the head and the replay would sit in the BufWriter, and
    // `curl -N` would show nothing until 8 KiB had piled up.
    body.flush()?;

    loop {
        let chunk = match events.recv_timeout(heartbeat) {
            Ok(event) => format_event(&event),
            Err(RecvTimeoutError::Timeout) => ": ping\n\n".to_string(),
            // Only if the hub dropped our Sender, which it never does today.
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Once the client is gone this write (or the next one) fails, `?`
        // returns, and `events` is dropped. The hub prunes us on its next send.
        body.write_all(chunk.as_bytes())?;
        body.flush()?;
    }
    body.finish()?;
    Ok(())
}

fn handle(socket: TcpStream, hub: &SharedHub, heartbeat: Duration) -> io::Result<()> {
    let peer = socket.peer_addr()?;
    // We only read before answering, so the read timeout only catches clients
    // that connect and stall. A long /events stream never reads, so it never
    // trips. The write timeout drops subscribers that stop reading.
    socket.set_read_timeout(Some(IO_TIMEOUT))?;
    socket.set_write_timeout(Some(IO_TIMEOUT))?;
    // Two handles to one socket: the BufReader owns the clone for reading.
    let mut reader = BufReader::new(socket.try_clone()?);
    let mut out = socket;

    let req = match read_head(&mut reader) {
        Ok(req) => req,
        Err(HeadError::Io(_)) => return Ok(()),
        Err(HeadError::TooLarge) => {
            eprintln!("{peer} request head too large");
            return reply(
                &mut out,
                "431 Request Header Fields Too Large",
                "text/plain",
                "head too large\n",
            );
        }
        Err(HeadError::Malformed(why)) => {
            eprintln!("{peer} bad request: {why}");
            return reply(
                &mut out,
                "400 Bad Request",
                "text/plain",
                &format!("{why}\n"),
            );
        }
    };
    eprintln!("{peer} {} {}", req.method, req.target);

    let (path, query) = req
        .target
        .split_once('?')
        .unwrap_or((req.target.as_str(), ""));
    match (req.method.as_str(), path) {
        ("GET", "/") => reply(&mut out, "200 OK", "text/html; charset=utf-8", INDEX_HTML),
        ("GET", "/events") => {
            let last_seen = req.header("last-event-id").and_then(|v| v.parse().ok());
            if let Err(e) = stream_events(out, hub, last_seen, heartbeat) {
                eprintln!("{peer} subscriber gone: {e}");
            }
            Ok(())
        }
        ("POST", "/publish") => {
            let (status, body) = publish(&req, query, &mut reader, hub);
            reply(&mut out, status, "text/plain; charset=utf-8", &body)
        }
        _ => reply(&mut out, "404 Not Found", "text/plain", "not found\n"),
    }
}

/// Accept loop: one thread per connection. A subscriber keeps its thread for
/// as long as it stays connected. Runs until the listener fails.
pub fn serve(listener: TcpListener, hub: SharedHub, heartbeat: Duration) {
    for socket in listener.incoming() {
        match socket {
            Ok(socket) => {
                let hub = Arc::clone(&hub);
                thread::spawn(move || {
                    if let Err(e) = handle(socket, &hub, heartbeat) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u64, data: &str) -> Event {
        Event {
            id,
            name: "message".into(),
            data: data.into(),
        }
    }

    #[test]
    fn formats_a_single_line_event() {
        // This exact string is 31 bytes: the "1f" chunk in the module docs.
        let text = format_event(&event(3, "hi"));
        assert_eq!(text, "id: 3\nevent: message\ndata: hi\n\n");
        assert_eq!(text.len(), 0x1f);
    }

    #[test]
    fn multiline_data_becomes_several_data_lines() {
        assert_eq!(
            format_event(&event(1, "a\r\nb\rc\n")),
            "id: 1\nevent: message\ndata: a\ndata: b\ndata: c\ndata: \n\n"
        );
    }

    #[test]
    fn event_names_come_from_the_query() {
        assert_eq!(event_name(""), Some("message"));
        assert_eq!(event_name("event=alert"), Some("alert"));
        for bad in [
            "event=",
            "event=a%0Ab",
            "event=a b",
            "name=x",
            "event=x&y=z",
        ] {
            assert_eq!(event_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn chunked_writer_frames_each_write() {
        let mut body = ChunkedWriter::new(Vec::new());
        body.write_all(b"hello").unwrap();
        body.write_all(&[b'x'; 26]).unwrap();
        body.write_all(b"").unwrap(); // must not emit an early "0" chunk
        let wire = body.finish().unwrap();
        let expected = [&b"5\r\nhello\r\n1a\r\n"[..], &[b'x'; 26], b"\r\n0\r\n\r\n"].concat();
        assert_eq!(wire, expected);
    }

    #[test]
    fn hub_numbers_events_and_keeps_only_recent_ones() {
        let mut hub = Hub::default();
        for i in 1..=105 {
            assert_eq!(hub.publish("message", &format!("m{i}")), i);
        }
        let (missed, _rx) = hub.subscribe(Some(0));
        assert_eq!(missed.len(), HISTORY);
        assert_eq!(missed[0].id, 6); // 1..=5 fell out of the ring buffer
    }

    #[test]
    fn replays_only_what_the_client_missed() {
        let mut hub = Hub::default();
        for _ in 0..5 {
            hub.publish("message", "x");
        }
        let ids = |(missed, _): (Vec<Event>, _)| missed.iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(hub.subscribe(Some(3))), [4, 5]);
        assert!(ids(hub.subscribe(None)).is_empty());
        assert!(ids(hub.subscribe(Some(99))).is_empty()); // e.g. an id from before a restart
    }

    #[test]
    fn live_events_arrive_and_dead_subscribers_are_pruned() {
        let mut hub = Hub::default();
        let (_, alive) = hub.subscribe(None);
        let (_, dead) = hub.subscribe(None);
        drop(dead); // what happens when a subscriber's thread exits
        hub.publish("message", "hi");
        assert_eq!(alive.try_recv().unwrap().data, "hi");
        assert_eq!(hub.subscriber_count(), 1);
    }

    #[test]
    fn parses_a_head_and_leaves_the_body_unread() {
        let mut input =
            &b"POST /publish?event=x HTTP/1.1\r\nHost: a\r\ncontent-LENGTH:  5 \r\n\r\nhello"[..];
        let req = read_head(&mut input).unwrap();
        assert_eq!(
            (req.method.as_str(), req.target.as_str()),
            ("POST", "/publish?event=x")
        );
        assert_eq!(req.header("Content-Length"), Some("5"));
        assert_eq!(input, b"hello");
    }

    #[test]
    fn accepts_bare_lf_line_endings() {
        let req = read_head(&mut &b"GET / HTTP/1.1\nHost: a\n\n"[..]).unwrap();
        assert_eq!(req.header("host"), Some("a"));
    }

    #[test]
    fn rejects_malformed_heads() {
        for bad in [
            "\r\n",
            "GET /\r\n\r\n",
            "GET / HTTP/1.1 extra\r\n\r\n",
            "GET / FTP/1.0\r\n\r\n",
            "GET / HTTP/1.1\r\nno colon\r\n\r\n",
            "GET / HTTP/1.1\r\nBad Name: x\r\n\r\n",
            "GET / HTTP/1.1\r\nA: b\r\n folded\r\n\r\n",
        ] {
            let result = read_head(&mut bad.as_bytes());
            assert!(matches!(result, Err(HeadError::Malformed(_))), "{bad:?}");
        }
    }

    #[test]
    fn rejects_oversized_and_truncated_heads() {
        let huge = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert!(matches!(
            read_head(&mut huge.as_bytes()),
            Err(HeadError::TooLarge)
        ));
        let partial = "GET / HTTP/1.1\r\nHost: a";
        assert!(matches!(
            read_head(&mut partial.as_bytes()),
            Err(HeadError::Io(_))
        ));
    }
}
