//! Integration tests against a canned HTTP server on a real localhost socket.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use fetchr::{Options, Url, fetch};

/// Answers each connection with a fixed reply chosen by path, then closes.
/// Returns the address and a counter of connections served.
fn canned_server() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            // Skip the headers, but read any body so closing doesn't RST.
            let mut body_len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim_end().is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    body_len = v.trim().parse().unwrap();
                }
            }
            reader.take(body_len).read_to_end(&mut Vec::new()).unwrap();

            let mut parts = request_line.split(' ');
            let (method, path) = (parts.next().unwrap(), parts.next().unwrap());
            let redirect = |code: &str, to: &str| {
                format!("HTTP/1.1 {code}\r\nLocation: {to}\r\nContent-Length: 0\r\n\r\n")
            };
            let reply = match path {
                // Extra bytes after the body must not end up in the output.
                "/fixed" => "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA".to_string(),
                "/chunked" => "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    5;note=x\r\nhello\r\n7\r\n, world\r\n0\r\nX-Trailer: yes\r\n\r\n"
                    .to_string(),
                "/close" => "HTTP/1.0 200 OK\r\n\r\nuntil the server hangs up".to_string(),
                "/short" => "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc".to_string(),
                "/hop1" => redirect("302 Found", "/hop2"),
                "/hop2" => redirect("301 Moved Permanently", &format!("http://{addr}/fixed")),
                "/form" => redirect("303 See Other", "method"), // relative
                "/keep" => redirect("307 Temporary Redirect", "/method"),
                "/loop" => redirect("302 Found", "/loop"),
                "/method" => format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{method}",
                    method.len()
                ),
                _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string(),
            };
            stream.write_all(reply.as_bytes()).unwrap();
        } // the stream drops here, closing the connection
    });
    (addr, hits)
}

/// Runs `fetch` and returns (result, stdout, stderr).
fn run(addr: SocketAddr, path: &str, opts: &Options) -> (std::io::Result<u16>, String, String) {
    let url: Url = format!("http://{addr}{path}").parse().unwrap();
    let (mut out, mut log) = (Vec::new(), Vec::new());
    let result = fetch(&url, opts, &mut out, &mut log);
    (
        result,
        String::from_utf8(out).unwrap(),
        String::from_utf8(log).unwrap(),
    )
}

fn follow() -> Options {
    Options {
        follow: true,
        ..Options::default()
    }
}

#[test]
fn reads_the_three_kinds_of_body() {
    let (addr, _) = canned_server();
    let plain = Options::default();
    assert_eq!(run(addr, "/fixed", &plain).1, "hello");
    assert_eq!(run(addr, "/chunked", &plain).1, "hello, world");
    assert_eq!(run(addr, "/close", &plain).1, "until the server hangs up");
}

#[test]
fn a_short_body_is_an_error() {
    let (addr, _) = canned_server();
    let (result, _, _) = run(addr, "/short", &Options::default());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("7 of 10 body bytes missing")
    );
}

#[test]
fn head_prints_headers_and_no_body() {
    let (addr, _) = canned_server();
    let opts = Options {
        head: true,
        ..Options::default()
    };
    let (result, out, _) = run(addr, "/fixed", &opts);
    assert_eq!(result.unwrap(), 200);
    assert_eq!(out, "HTTP/1.1 200 OK\nContent-Length: 5\n\n");
}

#[test]
fn verbose_logs_both_directions() {
    let (addr, _) = canned_server();
    let opts = Options {
        verbose: true,
        ..Options::default()
    };
    let (_, out, log) = run(addr, "/fixed", &opts);
    assert_eq!(out, "hello");
    assert!(log.contains("> GET /fixed HTTP/1.1\n"), "{log}");
    assert!(log.contains(&format!("> Host: {addr}\n")), "{log}");
    assert!(
        log.contains("< HTTP/1.1 200 OK\n< Content-Length: 5\n<\n"),
        "{log}"
    );
}

#[test]
fn follows_a_redirect_chain_only_with_l() {
    let (addr, hits) = canned_server();
    let (result, out, _) = run(addr, "/hop1", &Options::default());
    assert_eq!((result.unwrap(), out.as_str()), (302, ""));

    let (result, out, _) = run(addr, "/hop1", &follow());
    assert_eq!((result.unwrap(), out.as_str()), (200, "hello"));
    assert_eq!(hits.load(Ordering::SeqCst), 1 + 3);
}

#[test]
fn see_other_switches_to_get_but_307_keeps_post() {
    let (addr, _) = canned_server();
    let post = Options {
        data: Some("x=1".into()),
        ..follow()
    };
    assert_eq!(run(addr, "/form", &post).1, "GET");
    assert_eq!(run(addr, "/keep", &post).1, "POST");
}

#[test]
fn a_redirect_loop_stops_at_the_limit() {
    let (addr, hits) = canned_server();
    let (result, _, _) = run(addr, "/loop", &follow());
    assert_eq!(
        result.unwrap_err().to_string(),
        "maximum (10) redirects followed"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 11); // the first request + 10 redirects
}
