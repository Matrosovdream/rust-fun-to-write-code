//! Integration tests over real localhost sockets.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use httpd::{Config, serve};
use tempfile::TempDir;

/// A root with a page, a subdirectory without index.html, and a file
/// *outside* the root that traversal attempts (and a symlink) would like
/// to reach.
fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("root/files")).unwrap();
    fs::write(dir.path().join("root/index.html"), "<h1>home</h1>").unwrap();
    fs::write(dir.path().join("root/style.css"), "body{}").unwrap();
    fs::write(dir.path().join("root/files/a b.txt"), "spaced").unwrap();
    fs::write(dir.path().join("secret.txt"), "top secret").unwrap();
    // A symlink inside the root that points outside it.
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        dir.path().join("secret.txt"),
        dir.path().join("root/link.txt"),
    )
    .unwrap();
    dir
}

/// Binds port 0 (the OS picks a free one) and serves in the background.
fn start(dir: &TempDir, idle: Duration) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let mut config = Config::new(dir.path().join("root"));
    config.idle_timeout = idle;
    thread::spawn(move || serve(listener, config));
    addr
}

fn connect(addr: SocketAddr) -> (BufReader<TcpStream>, TcpStream) {
    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    (BufReader::new(stream.try_clone().unwrap()), stream)
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        let found = self
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name));
        found.map(|(_, v)| v.as_str())
    }
}

/// Just enough of an HTTP client: status line, headers, Content-Length body.
fn read_reply(r: &mut impl BufRead, head_only: bool) -> Reply {
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line.split(' ').nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        line.clear();
        r.read_line(&mut line).unwrap();
        match line.trim_end().split_once(": ") {
            Some((n, v)) => headers.push((n.to_string(), v.to_string())),
            None => break,
        }
    }
    let mut reply = Reply {
        status,
        headers,
        body: Vec::new(),
    };
    let len: u64 = reply.header("content-length").unwrap().parse().unwrap();
    if !head_only {
        r.take(len).read_to_end(&mut reply.body).unwrap();
    }
    reply
}

/// Sends one raw request and reads one reply.
fn exchange(addr: SocketAddr, raw: &str) -> Reply {
    let (mut reader, mut writer) = connect(addr);
    writer.write_all(raw.as_bytes()).unwrap();
    read_reply(&mut reader, raw.starts_with("HEAD"))
}

fn get(addr: SocketAddr, target: &str) -> Reply {
    exchange(
        addr,
        &format!("GET {target} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"),
    )
}

#[test]
fn serves_files_with_content_type_and_head() {
    let dir = fixture();
    let addr = start(&dir, Duration::from_secs(5));

    let page = get(addr, "/");
    assert_eq!(page.status, 200);
    assert_eq!(page.body, b"<h1>home</h1>");
    assert_eq!(
        page.header("content-type"),
        Some("text/html; charset=utf-8")
    );

    let head = exchange(addr, "HEAD /style.css HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(head.header("content-type"), Some("text/css; charset=utf-8"));
    assert_eq!(head.header("content-length"), Some("6"));
    assert!(head.body.is_empty());

    assert_eq!(get(addr, "/files/a%20b.txt").body, b"spaced");
}

#[test]
fn two_requests_share_one_kept_alive_socket() {
    let dir = fixture();
    let addr = start(&dir, Duration::from_secs(5));
    let (mut reader, mut writer) = connect(addr);

    writer
        .write_all(b"GET /style.css HTTP/1.1\r\nHost: t\r\n\r\n")
        .unwrap();
    let first = read_reply(&mut reader, false);
    assert_eq!((first.status, first.header("connection")), (200, None));

    writer
        .write_all(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
        .unwrap();
    let second = read_reply(&mut reader, false);
    assert_eq!(second.body, b"<h1>home</h1>");
    assert_eq!(second.header("connection"), Some("close"));

    let mut rest = Vec::new();
    assert_eq!(reader.read_to_end(&mut rest).unwrap(), 0); // server closed
}

#[test]
fn http_1_0_closes_after_one_response() {
    let dir = fixture();
    let addr = start(&dir, Duration::from_secs(5));
    let (mut reader, mut writer) = connect(addr);
    writer.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
    assert_eq!(read_reply(&mut reader, false).status, 200);
    assert_eq!(reader.read_to_end(&mut Vec::new()).unwrap(), 0);
}

#[test]
fn directories_redirect_then_list() {
    let dir = fixture();
    let addr = start(&dir, Duration::from_secs(5));

    let moved = get(addr, "/files");
    assert_eq!(
        (moved.status, moved.header("location")),
        (301, Some("/files/"))
    );
    // Never `//files/`: a browser would read that as a host name.
    assert_eq!(get(addr, "//files").header("location"), Some("/files/"));

    let listing = String::from_utf8(get(addr, "/files/").body).unwrap();
    assert!(
        listing.contains("<a href=\"a%20b.txt\">a b.txt</a>"),
        "{listing}"
    );
}

#[test]
fn error_statuses() {
    let dir = fixture();
    let addr = start(&dir, Duration::from_secs(5));

    assert_eq!(get(addr, "/nope.html").status, 404);
    assert_eq!(get(addr, "/../secret.txt").status, 403);
    assert_eq!(get(addr, "/files/%2e%2e/%2e%2e/secret.txt").status, 403);
    #[cfg(unix)]
    assert_eq!(get(addr, "/link.txt").status, 403);

    let post = exchange(
        addr,
        "POST / HTTP/1.1\r\nHost: t\r\nContent-Length: 2\r\n\r\nhi",
    );
    assert_eq!(
        (post.status, post.header("allow")),
        (405, Some("GET, HEAD"))
    );

    assert_eq!(exchange(addr, "hello\r\n\r\n").status, 400);
    let big = format!(
        "GET / HTTP/1.1\r\nHost: t\r\nX-Big: {}\r\n\r\n",
        "a".repeat(20_000)
    );
    let reply = exchange(addr, &big);
    assert_eq!(
        (reply.status, reply.header("connection")),
        (431, Some("close"))
    );
}

#[test]
fn idle_connections_are_closed() {
    let dir = fixture();
    let addr = start(&dir, Duration::from_millis(100));
    let (mut reader, _writer) = connect(addr);
    // We send nothing; the server's read times out and it hangs up.
    assert_eq!(reader.read_to_end(&mut Vec::new()).unwrap(), 0);
}
