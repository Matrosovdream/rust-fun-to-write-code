//! Integration tests over real localhost sockets.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use jsonrpc::{MAX_LINE, default_registry, serve};
use serde_json::{Value, json};

/// Binds port 0 (the OS picks a free one) and serves in the background.
fn start_server(idle_timeout: Duration) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, Arc::new(default_registry()), idle_timeout));
    addr
}

struct Conn {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Conn {
    fn open(addr: SocketAddr) -> Conn {
        let stream = TcpStream::connect(addr).unwrap();
        // Keeps a missing reply from hanging the test.
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        Conn {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
        }
    }

    fn send(&mut self, line: &str) {
        writeln!(self.writer, "{line}").unwrap();
    }

    /// `None` when the server closed the connection.
    fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line).unwrap() {
            0 => None,
            _ => Some(serde_json::from_str(&line).unwrap()),
        }
    }
}

#[test]
fn request_and_reply_over_a_socket() {
    let mut conn = Conn::open(start_server(Duration::from_secs(5)));
    conn.send(r#"{"jsonrpc":"2.0","method":"add","params":[1,2],"id":1}"#);
    assert_eq!(
        conn.recv(),
        Some(json!({"jsonrpc": "2.0", "result": 3, "id": 1}))
    );
    conn.send(
        r#"{"jsonrpc":"2.0","method":"subtract","params":{"minuend":42,"subtrahend":23},"id":"x"}"#,
    );
    assert_eq!(
        conn.recv(),
        Some(json!({"jsonrpc": "2.0", "result": 19, "id": "x"}))
    );
}

#[test]
fn notifications_get_no_reply_line() {
    let mut conn = Conn::open(start_server(Duration::from_secs(5)));
    // If either notification produced a line, it would be read here
    // instead of the reply to id 2.
    conn.send(r#"{"jsonrpc":"2.0","method":"echo","params":[1]}"#);
    conn.send(r#"[{"jsonrpc":"2.0","method":"nope"},{"jsonrpc":"2.0","method":"echo"}]"#);
    conn.send("");
    conn.send(r#"{"jsonrpc":"2.0","method":"echo","params":["hi"],"id":2}"#);
    assert_eq!(
        conn.recv(),
        Some(json!({"jsonrpc": "2.0", "result": ["hi"], "id": 2}))
    );
}

#[test]
fn counter_is_shared_between_connections() {
    let addr = start_server(Duration::from_secs(5));
    let incr = r#"{"jsonrpc":"2.0","method":"counter.incr","id":1}"#;
    let mut a = Conn::open(addr);
    let mut b = Conn::open(addr);
    a.send(incr);
    assert_eq!(a.recv().unwrap()["result"], 1);
    b.send(incr);
    assert_eq!(b.recv().unwrap()["result"], 2);
}

#[test]
fn garbage_gets_errors_and_the_connection_survives() {
    let mut conn = Conn::open(start_server(Duration::from_secs(5)));
    conn.send("this is not json");
    assert_eq!(conn.recv().unwrap()["error"]["code"], -32700);
    conn.writer.write_all(b"\xff\xfe{}\n").unwrap();
    assert_eq!(conn.recv().unwrap()["error"]["code"], -32700);
    conn.send(r#"{"jsonrpc":"2.0","method":"add","params":[2,2],"id":3}"#);
    assert_eq!(conn.recv().unwrap()["result"], 4);
}

#[test]
fn overlong_line_gets_an_error_then_close() {
    let mut conn = Conn::open(start_server(Duration::from_secs(5)));
    // Exactly MAX_LINE bytes and no newline: the server reads all of it, so
    // closing afterwards is a clean FIN rather than a reset.
    conn.writer.write_all(&vec![b'x'; MAX_LINE]).unwrap();
    let reply = conn.recv().unwrap();
    assert_eq!(reply["error"]["code"], -32700);
    assert_eq!(reply["error"]["data"], "line too long");
    assert_eq!(conn.recv(), None);
}

#[test]
fn silent_client_is_disconnected() {
    let mut conn = Conn::open(start_server(Duration::from_millis(100)));
    assert_eq!(conn.recv(), None);
}
