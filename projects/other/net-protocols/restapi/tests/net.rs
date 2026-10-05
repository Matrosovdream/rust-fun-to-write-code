//! End-to-end tests over real localhost sockets.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use restapi::middleware::{Logger, RateLimiter};
use restapi::router::Router;
use restapi::serve;
use restapi::todos::{SharedStore, routes};
use serde_json::{Value, json};

/// Binds port 0 (the OS picks a free one) and serves in the background.
fn start(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, router, 2));
    addr
}

/// One kept-alive connection to send several requests over.
struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    fn connect(addr: SocketAddr) -> Client {
        let stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Client {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
        }
    }

    /// Sends a request and returns (status, headers, JSON body or Null).
    fn send(&mut self, method: &str, path: &str, body: &str) -> (u16, Vec<String>, Value) {
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        self.writer.write_all((head + body).as_bytes()).unwrap();

        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        let status = line.split(' ').nth(1).unwrap().parse().unwrap();
        let (mut headers, mut len) = (Vec::new(), 0);
        loop {
            line.clear();
            self.reader.read_line(&mut line).unwrap();
            let header = line.trim_end().to_string();
            if header.is_empty() {
                break;
            }
            if let Some(n) = header.strip_prefix("Content-Length: ") {
                len = n.parse().unwrap();
            }
            headers.push(header);
        }
        let mut body = Vec::new();
        (&mut self.reader).take(len).read_to_end(&mut body).unwrap();
        let json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        (status, headers, json)
    }
}

#[test]
fn todo_crud_over_one_kept_alive_connection() {
    let addr = start(routes(SharedStore::default()).wrap(Logger));
    let mut client = Client::connect(addr);

    assert_eq!(client.send("GET", "/todos", "").2, json!([]));

    let (status, headers, todo) = client.send("POST", "/todos", r#"{"title":"write a router"}"#);
    assert_eq!(status, 201);
    assert!(
        headers.contains(&"Location: /todos/1".to_string()),
        "{headers:?}"
    );
    assert_eq!(
        todo,
        json!({ "id": 1, "title": "write a router", "done": false })
    );

    let (status, _, todo) = client.send("PATCH", "/todos/1", r#"{"done":true}"#);
    assert_eq!((status, todo["done"].clone()), (200, json!(true)));

    let (status, headers, body) = client.send("DELETE", "/todos/1", "");
    assert_eq!((status, body), (204, Value::Null));
    assert!(
        !headers.iter().any(|h| h.starts_with("Content-Length")),
        "{headers:?}"
    );

    assert_eq!(
        client.send("GET", "/todos/1", "").2,
        json!({ "error": "not found" })
    );
    assert_eq!(client.send("POST", "/todos", "{oops").0, 400);
    let (status, headers, _) = client.send("PUT", "/todos/1", "");
    assert_eq!(status, 405);
    assert!(
        headers.contains(&"Allow: GET, PATCH, DELETE".to_string()),
        "{headers:?}"
    );
}

#[test]
fn rate_limit_trips_with_429_and_retry_after() {
    // A frozen clock: no tokens ever refill, however slow the machine is.
    let t0 = Instant::now();
    let limiter = RateLimiter::with_clock(3, 1, move || t0);
    let addr = start(routes(SharedStore::default()).wrap(limiter));
    let mut client = Client::connect(addr);
    let statuses: Vec<u16> = (0..5).map(|_| client.send("GET", "/todos", "").0).collect();
    assert_eq!(statuses, [200, 200, 200, 429, 429]);
    let (_, headers, body) = client.send("GET", "/todos", "");
    assert!(
        headers.contains(&"Retry-After: 1".to_string()),
        "{headers:?}"
    );
    assert_eq!(body, json!({ "error": "too many requests" }));
}

#[test]
fn garbage_gets_400_and_a_closed_connection() {
    let addr = start(routes(SharedStore::default()));
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(b"\x16\x03\x01 not http\r\n\r\n").unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap(); // returns once the server closes
    assert!(reply.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{reply}");
    assert!(reply.contains("Connection: close"), "{reply}");
}
