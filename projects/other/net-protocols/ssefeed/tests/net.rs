//! Integration tests over real localhost sockets.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ssefeed::{Hub, SharedHub, serve};

/// Binds port 0 (the OS picks a free one) and serves in a background thread.
fn start(heartbeat: Duration) -> (SocketAddr, SharedHub) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hub: SharedHub = Arc::new(Mutex::new(Hub::default()));
    let server_hub = Arc::clone(&hub);
    thread::spawn(move || serve(listener, server_hub, heartbeat));
    (addr, hub)
}

/// Sends a raw request. Returns the response head and a reader positioned at the body.
fn request(addr: SocketAddr, raw: &str) -> (String, BufReader<TcpStream>) {
    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    (&stream).write_all(raw.as_bytes()).unwrap();
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            return (head, reader);
        }
        head.push_str(&line);
    }
}

fn subscribe(addr: SocketAddr, extra_headers: &str) -> BufReader<TcpStream> {
    let (head, reader) = request(
        addr,
        &format!("GET /events HTTP/1.1\r\nHost: t\r\n{extra_headers}\r\n"),
    );
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
    assert!(head.contains("Content-Type: text/event-stream\r\n"));
    assert!(head.contains("Transfer-Encoding: chunked\r\n"));
    reader
}

/// Reads one chunk of a chunked body: hex size line, data, CRLF.
fn read_chunk(reader: &mut impl BufRead) -> String {
    let mut size = String::new();
    reader.read_line(&mut size).unwrap();
    let size = usize::from_str_radix(size.trim_end(), 16).unwrap();
    let mut data = vec![0; size + 2];
    reader.read_exact(&mut data).unwrap();
    assert!(data.ends_with(b"\r\n"));
    data.truncate(size);
    String::from_utf8(data).unwrap()
}

fn publish(addr: SocketAddr, target: &str, body: &str) -> String {
    let raw = format!(
        "POST {target} HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (head, mut reader) = request(addr, &raw);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    head + &text
}

#[test]
fn serves_the_page_as_html() {
    let (addr, _) = start(Duration::from_secs(60));
    let (head, mut reader) = request(addr, "GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert!(head.contains("Content-Type: text/html; charset=utf-8\r\n"));
    let mut page = String::new();
    reader.read_to_string(&mut page).unwrap();
    assert!(page.contains("new EventSource('/events')"));
}

#[test]
fn subscriber_gets_each_event_as_one_chunk() {
    let (addr, _) = start(Duration::from_secs(60));
    // The response head is written after the hub registered us, so
    // publishing once we've seen it can't race with the subscription.
    let mut events = subscribe(addr, "");
    let reply = publish(addr, "/publish", "hello\nworld");
    assert!(reply.starts_with("HTTP/1.1 200 OK") && reply.ends_with("published event 1\n"));
    assert_eq!(
        read_chunk(&mut events),
        "id: 1\nevent: message\ndata: hello\ndata: world\n\n"
    );

    publish(addr, "/publish?event=alert", "disk full");
    assert_eq!(
        read_chunk(&mut events),
        "id: 2\nevent: alert\ndata: disk full\n\n"
    );
}

#[test]
fn last_event_id_replays_missed_events() {
    let (addr, hub) = start(Duration::from_secs(60));
    for text in ["one", "two", "three"] {
        hub.lock().unwrap().publish("message", text);
    }
    let mut events = subscribe(addr, "Last-Event-ID: 1\r\n");
    assert!(read_chunk(&mut events).starts_with("id: 2\n"));
    assert!(read_chunk(&mut events).starts_with("id: 3\n"));
}

#[test]
fn idle_streams_get_heartbeats() {
    let (addr, _) = start(Duration::from_millis(20));
    let mut events = subscribe(addr, "");
    assert_eq!(read_chunk(&mut events), ": ping\n\n");
}

#[test]
fn subscribers_that_hang_up_are_pruned() {
    let (addr, hub) = start(Duration::from_millis(20));
    drop(subscribe(addr, ""));
    // The server notices on a failed heartbeat write; the hub then drops the
    // dead Sender on its next publish. Poll until that happened.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut hub = hub.lock().unwrap();
        hub.publish("message", "anyone there?");
        if hub.subscriber_count() == 0 {
            break;
        }
        drop(hub);
        assert!(Instant::now() < deadline, "subscriber was never pruned");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn bad_requests_get_error_statuses() {
    let (addr, _) = start(Duration::from_secs(60));
    // Every case here lets the server read all we send. Closing a socket with
    // unread input triggers a reset that can eat the reply, so the oversized
    // head case lives in the unit tests instead.
    let too_big = format!(
        "POST /publish HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        1 << 20
    );
    let cases = [
        ("nonsense\r\n\r\n", "400"),
        ("POST /publish HTTP/1.1\r\n\r\n", "411"),
        (too_big.as_str(), "413"),
        (
            "POST /publish?event=a+b HTTP/1.1\r\nContent-Length: 1\r\n\r\nx",
            "400",
        ),
        ("GET /nope HTTP/1.1\r\n\r\n", "404"),
    ];
    for (raw, status) in cases {
        let (head, _) = request(addr, raw);
        assert!(
            head.starts_with(&format!("HTTP/1.1 {status} ")),
            "{raw:?} -> {head}"
        );
    }
}
