//! Integration tests over real localhost sockets: a proxy, plus small
//! in-test upstreams (an HTTP origin and an echo server).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use httpproxy::{Config, serve};

fn start_proxy(blocked: &[&str]) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Config {
        blocked: blocked.iter().map(|b| b.to_string()).collect(),
        connect_timeout: Duration::from_secs(2),
    };
    thread::spawn(move || serve(listener, config));
    addr
}

/// A one-shot origin server. It reports the request it received (head and
/// body) on the channel, then answers with `response` and closes.
fn start_origin(response: &'static str) -> (SocketAddr, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut request = String::new();
        let mut body_len = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(len) = line.strip_prefix("Content-Length: ") {
                body_len = len.trim().parse().unwrap();
            }
            request.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let mut body = vec![0; body_len];
        reader.read_exact(&mut body).unwrap();
        request.push_str(&String::from_utf8(body).unwrap());
        tx.send(request).unwrap();
        socket.write_all(response.as_bytes()).unwrap();
    });
    (addr, rx)
}

/// Sends `raw` through the proxy and returns everything it answers.
fn through_proxy(proxy: SocketAddr, raw: &str) -> String {
    let mut stream = TcpStream::connect(proxy).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn forwards_absolute_form_as_origin_form() {
    let (origin, seen) = start_origin("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
    let proxy = start_proxy(&[]);
    let response = through_proxy(
        proxy,
        &format!(
            "POST http://{origin}/echo?x=1 HTTP/1.1\r\nHost: {origin}\r\n\
             Proxy-Connection: Keep-Alive\r\nContent-Length: 5\r\n\r\nhello"
        ),
    );
    assert_eq!(response, "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");

    let request = seen.recv().unwrap();
    assert!(
        request.starts_with("POST /echo?x=1 HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(request.contains(&format!("Host: {origin}\r\n")));
    assert!(request.contains("Via: 1.1 httpproxy\r\n"));
    assert!(request.contains("Connection: close\r\n"));
    assert!(!request.contains("Proxy-Connection"));
    assert!(request.ends_with("\r\n\r\nhello"));
}

#[test]
fn connect_tunnels_bytes_both_ways_and_passes_half_close() {
    // Echo server: copies everything back, then closes when the client
    // half-closes its side.
    let echo = TcpListener::bind("127.0.0.1:0").unwrap();
    let echo_addr = echo.local_addr().unwrap();
    thread::spawn(move || {
        let (socket, _) = echo.accept().unwrap();
        std::io::copy(&mut &socket, &mut &socket).unwrap();
    });
    let proxy = start_proxy(&[]);

    let mut client = TcpStream::connect(proxy).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    // "early" rides in the same packet as the CONNECT head, the way an eager
    // TLS client sends its ClientHello. The proxy must not lose it.
    write!(
        client,
        "CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\n\r\nearly "
    )
    .unwrap();
    let established = b"HTTP/1.1 200 Connection Established\r\n\r\n";
    let mut reply = vec![0; established.len()];
    client.read_exact(&mut reply).unwrap();
    assert_eq!(reply, established);

    client.write_all(b"bytes").unwrap();
    client.shutdown(Shutdown::Write).unwrap(); // "I'm done sending"
    let mut echoed = String::new();
    client.read_to_string(&mut echoed).unwrap(); // ends only if EOF made it back
    assert_eq!(echoed, "early bytes");
}

#[test]
fn refusals_get_the_right_status() {
    let proxy = start_proxy(&["blocked.test"]);
    // Nothing listens on this port once the listener is dropped.
    let dead = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let cases = [
        (
            "GET http://blocked.test/ HTTP/1.1\r\n\r\n".to_string(),
            "403 Forbidden",
        ),
        (
            "GET http://www.blocked.test/ HTTP/1.1\r\n\r\n".to_string(),
            "403 Forbidden",
        ),
        (
            "GET /not-a-proxy-request HTTP/1.1\r\n\r\n".to_string(),
            "400 Bad Request",
        ),
        (
            "CONNECT blocked.test HTTP/1.1\r\n\r\n".to_string(),
            "400 Bad Request",
        ),
        ("this is not http\r\n\r\n".to_string(), "400 Bad Request"),
        (
            format!("GET http://{dead}/ HTTP/1.1\r\n\r\n"),
            "502 Bad Gateway",
        ),
        (
            format!("CONNECT {dead} HTTP/1.1\r\n\r\n"),
            "502 Bad Gateway",
        ),
    ];
    for (raw, status) in cases {
        let response = through_proxy(proxy, &raw);
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}\r\n")),
            "{raw:?} -> {response}"
        );
    }
}
