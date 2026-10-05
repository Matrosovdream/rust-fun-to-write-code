//! httpproxy — a forward HTTP proxy with CONNECT tunnels.
//!
//! A forward proxy sits between a client and *any* server, and the client
//! tells it where to go. For plain HTTP the request line carries the whole
//! URL (absolute-form, RFC 9112 §3.2.2) instead of just the path. The proxy
//! rewrites it to the usual origin-form, drops the headers that only
//! described the client-to-proxy connection, and streams the answer back:
//!
//! ```text
//! C->P: GET http://example.com/page?q=1 HTTP/1.1   <- absolute-form
//! C->P: Host: example.com
//! C->P: Proxy-Connection: Keep-Alive               <- hop-by-hop: dropped
//! C->P:
//! P->S: GET /page?q=1 HTTP/1.1                     <- origin-form
//! P->S: Host: example.com
//! P->S: Via: 1.1 httpproxy                         <- "a proxy was here" (RFC 9110 §7.6.3)
//! P->S: Connection: close                          <- one request per connection
//! P->S:
//! S->P->C: HTTP/1.1 200 OK ...                     <- copied back byte for byte
//! ```
//!
//! HTTPS is encrypted end to end, so the proxy can't read or rewrite
//! anything. The client asks for a raw tunnel instead (CONNECT, RFC 9110
//! §9.3.6). After the 200 the proxy just copies bytes both ways:
//!
//! ```text
//! C->P: CONNECT example.com:443 HTTP/1.1           <- authority-form
//! C->P: Host: example.com:443
//! C->P:
//! P->C: HTTP/1.1 200 Connection Established
//! P->C:
//! C<->P<->S: TLS handshake, then encrypted bytes   <- opaque to the proxy
//! ```
//!
//! Hop-by-hop headers (RFC 9110 §7.6.1) describe a single connection, not the
//! message, so a proxy must not forward them: `Connection`, `Keep-Alive`,
//! `Proxy-Connection`, `Proxy-Authorization`, `Proxy-Authenticate`, `TE`,
//! `Trailer`, `Transfer-Encoding`, `Upgrade`, plus any header that
//! `Connection` names.
//!
//! Errors: 400 for a malformed request or target, 403 for a host on the
//! `--block` list, 411 for a chunked request body (unsupported here), 502
//! when the upstream can't be reached or answers garbage, 504 when it times
//! out.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Most bytes we accept for a request line plus headers.
pub const MAX_HEAD: usize = 8 * 1024;
/// How long to wait for a TCP connection to the upstream before giving up (504).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a client may take to send its request head and body.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the upstream may stay silent while we wait for its response.
/// Tunnels get no timeout: an idle tunnel is normal.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

/// Headers that describe one connection rather than the message.
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

pub struct Config {
    /// Hosts to refuse with 403. Each entry also blocks its subdomains.
    pub blocked: Vec<String>,
    pub connect_timeout: Duration,
}

impl Config {
    /// `example.com` blocks `example.com` and `www.example.com`, but not
    /// `notexample.com`. That's why the suffix check includes the dot.
    pub fn is_blocked(&self, host: &str) -> bool {
        self.blocked.iter().any(|entry| {
            let entry = entry.to_ascii_lowercase();
            host == entry || host.ends_with(&format!(".{entry}"))
        })
    }
}

/// Why we won't (or can't) proxy a request. It becomes the error response.
#[derive(Debug, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub message: String,
}

impl Refusal {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Refusal {
            status,
            message: message.into(),
        }
    }
}

// ------------------------------------------------------------------ parsing

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    /// Header names are case-insensitive (RFC 9110 §5.1).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers_named(name).next()
    }

    fn headers_named<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a str> {
        self.headers
            .iter()
            .filter(move |(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug)]
pub enum HeadError {
    /// The socket failed, timed out, or closed early: nobody left to answer.
    Io(io::Error),
    TooLarge,
    Malformed(&'static str),
}

impl From<io::Error> for HeadError {
    fn from(e: io::Error) -> Self {
        HeadError::Io(e)
    }
}

/// Reads the request line and headers up to the blank line (RFC 9112 §2.1).
/// Whatever follows (a body, or the first bytes of a tunnel) stays in `reader`.
pub fn read_head(reader: &mut impl BufRead) -> Result<Request, HeadError> {
    let mut limited = reader.take(MAX_HEAD as u64);
    let mut lines = Vec::new();
    loop {
        let mut raw = Vec::new();
        limited.read_until(b'\n', &mut raw)?;
        if raw.last() != Some(&b'\n') {
            return Err(if limited.limit() == 0 {
                HeadError::TooLarge
            } else {
                HeadError::Io(io::ErrorKind::UnexpectedEof.into())
            });
        }
        let line = String::from_utf8(raw).map_err(|_| HeadError::Malformed("not UTF-8"))?;
        let line = line.trim_end_matches(['\r', '\n']); // CRLF, or a bare LF
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

/// Where a request should go.
#[derive(Debug, PartialEq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    /// Path and query to send upstream, or `None` for a CONNECT tunnel.
    pub path: Option<String>,
}

/// Parses the request target. A proxy accepts two of its forms (RFC 9112
/// §3.2): `CONNECT host:port` and `GET http://host[:port]/path?query`.
pub fn parse_target(method: &str, target: &str) -> Result<Target, &'static str> {
    if method == "CONNECT" {
        let (host, port) = split_host_port(target, None).ok_or("CONNECT needs host:port")?;
        return Ok(Target {
            host,
            port,
            path: None,
        });
    }
    let rest = target
        .strip_prefix("http://")
        .ok_or("expected an absolute http:// URL (https goes through CONNECT)")?;
    // The authority ends at the first '/' or '?'.
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let (host, port) = split_host_port(authority, Some(80)).ok_or("bad host or port")?;
    Ok(Target {
        host,
        port,
        path: Some(path),
    })
}

/// Splits `host[:port]`. Hosts are restricted to letters, digits, '.', '-'
/// and '_'. That rules out user:password@ prefixes and IPv6 literals, both
/// out of scope here.
fn split_host_port(authority: &str, default_port: Option<u16>) -> Option<(String, u16)> {
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (authority, default_port?),
    };
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b));
    valid.then(|| (host.to_ascii_lowercase(), port))
}

/// The request body length. Two different `Content-Length` values are a
/// classic request-smuggling trick (RFC 9112 §6.3), so refuse them.
fn content_length(req: &Request) -> Result<u64, &'static str> {
    let mut values = req.headers_named("content-length");
    let Some(first) = values.next() else {
        return Ok(0);
    };
    if values.any(|other| other != first) {
        return Err("conflicting Content-Length headers");
    }
    first.parse().map_err(|_| "bad Content-Length")
}

/// Decides what to do with a request before touching the network: where it
/// goes and how long its body is, or why we refuse it.
pub fn plan(req: &Request, config: &Config) -> Result<(Target, u64), Refusal> {
    let target = parse_target(&req.method, &req.target).map_err(|why| Refusal::new(400, why))?;
    if config.is_blocked(&target.host) {
        return Err(Refusal::new(
            403,
            format!("{} is blocked by this proxy", target.host),
        ));
    }
    if req.header("transfer-encoding").is_some() {
        return Err(Refusal::new(
            411,
            "chunked request bodies are not supported; send Content-Length",
        ));
    }
    let body_len = content_length(req).map_err(|why| Refusal::new(400, why))?;
    Ok((target, body_len))
}

/// Builds the head we send upstream: an origin-form request line, the
/// end-to-end headers, then our own `Host`, `Via` and `Connection`.
pub fn rewrite_head(req: &Request, target: &Target, path: &str) -> String {
    // `Connection: X-Foo` makes X-Foo hop-by-hop as well.
    let named: Vec<String> = req
        .headers_named("connection")
        .flat_map(|value| value.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();
    let mut head = format!("{} {path} HTTP/1.1\r\n", req.method);
    for (name, value) in &req.headers {
        let lower = name.to_ascii_lowercase();
        // Host is replaced below: it must match the URL we were given (RFC 9112 §3.2.2).
        if HOP_BY_HOP.contains(&lower.as_str()) || named.contains(&lower) || lower == "host" {
            continue;
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let host = match target.port {
        80 => target.host.clone(),
        port => format!("{}:{port}", target.host),
    };
    head.push_str(&format!(
        "Host: {host}\r\nVia: 1.1 httpproxy\r\nConnection: close\r\n\r\n"
    ));
    head
}

/// The status code from a response's first line, e.g. `HTTP/1.1 404 Not Found`.
pub fn parse_status(line: &[u8]) -> Option<u16> {
    let mut parts = std::str::from_utf8(line).ok()?.split_whitespace();
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    parts
        .next()?
        .parse()
        .ok()
        .filter(|code| (100..600).contains(code))
}

/// Maps an upstream failure to a refusal: timeouts are 504, the rest 502.
pub fn upstream_error(e: io::Error) -> Refusal {
    match e.kind() {
        // A read timeout surfaces as WouldBlock on Unix and TimedOut on Windows.
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            Refusal::new(504, "upstream timed out")
        }
        _ => Refusal::new(502, format!("upstream failed: {e}")),
    }
}

// ------------------------------------------------------------------ network

/// Connects to host:port, trying each address DNS returns (often an IPv6 one
/// and an IPv4 one). Every attempt gets a timeout. DNS itself has none in std.
fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, Refusal> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| Refusal::new(502, format!("can't resolve {host}: {e}")))?;
    let mut last = Refusal::new(502, format!("{host} has no addresses"));
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = upstream_error(e),
        }
    }
    Err(last)
}

/// Copies bytes both ways between two sockets until both directions are
/// done, and returns how many went each way. `io::copy` blocks, so one
/// direction runs on a helper thread. When one side stops sending, we
/// half-close the other side (`shutdown(Write)`). That passes the EOF along
/// while replies can still flow back.
pub fn splice(a: TcpStream, b: TcpStream) -> io::Result<(u64, u64)> {
    let (mut a_read, mut b_write) = (a.try_clone()?, b.try_clone()?);
    let a_to_b = thread::spawn(move || {
        let copied = io::copy(&mut a_read, &mut b_write);
        let _ = b_write.shutdown(Shutdown::Write);
        copied
    });
    let (mut b_read, mut a_write) = (b, a);
    let b_to_a = io::copy(&mut b_read, &mut a_write);
    let _ = a_write.shutdown(Shutdown::Write);
    let a_to_b = a_to_b.join().expect("io::copy does not panic");
    Ok((a_to_b?, b_to_a?))
}

/// CONNECT: answer 200, then splice client and upstream until both are done.
fn tunnel(client: BufReader<TcpStream>, upstream: TcpStream) -> io::Result<(u64, u64)> {
    client
        .get_ref()
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?;
    // A client may send its first bytes (a TLS ClientHello) right behind the
    // CONNECT head without waiting for our 200. Those bytes are already in
    // the BufReader's buffer, so forward them before splicing the sockets.
    let early = client.buffer();
    (&upstream).write_all(early)?;
    let early = early.len() as u64;
    let client = client.into_inner();
    // An idle tunnel is normal (think SSH), so drop the head-reading timeout.
    client.set_read_timeout(None)?;
    let (up, down) = splice(client, upstream)?;
    Ok((early + up, down))
}

/// Plain HTTP: sends the rewritten head and the body upstream, then streams
/// the response back. Returns the upstream status for the access log.
fn forward(
    req: &Request,
    target: &Target,
    path: &str,
    body_len: u64,
    client: &mut BufReader<TcpStream>,
    to_client: &mut TcpStream,
    upstream: TcpStream,
) -> Result<u16, Refusal> {
    upstream
        .set_read_timeout(Some(UPSTREAM_TIMEOUT))
        .map_err(upstream_error)?;
    // `&TcpStream` implements Read and Write, so no clone is needed here.
    let mut to_upstream = &upstream;
    to_upstream
        .write_all(rewrite_head(req, target, path).as_bytes())
        .map_err(upstream_error)?;
    // Stream exactly body_len bytes. The body never sits in memory whole.
    io::copy(&mut client.take(body_len), &mut to_upstream).map_err(upstream_error)?;

    // Read the status line ourselves so we can log it, then pass it on.
    let mut from_upstream = BufReader::new(&upstream);
    let mut status_line = Vec::new();
    (&mut from_upstream)
        .take(MAX_HEAD as u64)
        .read_until(b'\n', &mut status_line)
        .map_err(upstream_error)?;
    let status = parse_status(&status_line)
        .ok_or_else(|| Refusal::new(502, "upstream sent no valid HTTP response"))?;
    // From here on the client is receiving the response. A failure can only
    // cut it short, not turn it into an error page, so it's ignored.
    let _ = to_client
        .write_all(&status_line)
        .and_then(|()| io::copy(&mut from_upstream, to_client));
    Ok(status)
}

/// Routes one request. Returns a summary for the access log, or a refusal
/// to send back.
fn proxy(
    req: &Request,
    mut client: BufReader<TcpStream>,
    to_client: &mut TcpStream,
    config: &Config,
) -> Result<String, Refusal> {
    let (target, body_len) = plan(req, config)?;
    let upstream = connect(&target.host, target.port, config.connect_timeout)?;
    match &target.path {
        Some(path) => forward(
            req,
            &target,
            path,
            body_len,
            &mut client,
            to_client,
            upstream,
        )
        .map(|s| s.to_string()),
        None => Ok(match tunnel(client, upstream) {
            Ok((up, down)) => format!("200 tunnel closed ({up} bytes up, {down} down)"),
            Err(e) => format!("200 tunnel ended: {e}"),
        }),
    }
}

fn send_refusal(out: &mut impl Write, refusal: &Refusal) -> io::Result<()> {
    let reason = match refusal.status {
        400 => "Bad Request",
        403 => "Forbidden",
        411 => "Length Required",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let body = format!("{}\n", refusal.message);
    let response = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        refusal.status,
        body.len()
    );
    out.write_all(response.as_bytes())
}

fn handle(socket: TcpStream, config: &Config) -> io::Result<()> {
    let peer = socket.peer_addr()?;
    // Covers the head and the request body. Tunnels lift it later.
    socket.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    let mut client = BufReader::new(socket.try_clone()?);
    let mut to_client = socket;

    let req = match read_head(&mut client) {
        Ok(req) => req,
        Err(HeadError::Io(_)) => return Ok(()), // silent or gone: nobody to answer
        Err(e) => {
            let refusal = match e {
                HeadError::TooLarge => Refusal::new(431, "request head too large"),
                _ => Refusal::new(400, format!("malformed request: {e:?}")),
            };
            eprintln!("{peer} -> {} {}", refusal.status, refusal.message);
            return send_refusal(&mut to_client, &refusal);
        }
    };
    let summary = match proxy(&req, client, &mut to_client, config) {
        Ok(summary) => summary,
        Err(refusal) => {
            send_refusal(&mut to_client, &refusal)?;
            format!("{} {}", refusal.status, refusal.message)
        }
    };
    eprintln!("{peer} {} {} -> {summary}", req.method, req.target);
    Ok(())
}

/// Accept loop: one thread per client connection (plus one more per tunnel).
pub fn serve(listener: TcpListener, config: Config) {
    // Arc: every connection thread shares the one Config, read-only.
    let config = Arc::new(config);
    for socket in listener.incoming() {
        match socket {
            Ok(socket) => {
                let config = Arc::clone(&config);
                thread::spawn(move || {
                    if let Err(e) = handle(socket, &config) {
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

    fn request(head: &str) -> Request {
        read_head(&mut head.as_bytes()).unwrap()
    }

    fn config(blocked: &[&str]) -> Config {
        Config {
            blocked: blocked.iter().map(|b| b.to_string()).collect(),
            connect_timeout: CONNECT_TIMEOUT,
        }
    }

    #[test]
    fn parses_absolute_and_authority_form_targets() {
        let http = |host: &str, port, path: &str| Target {
            host: host.into(),
            port,
            path: Some(path.into()),
        };
        assert_eq!(
            parse_target("GET", "http://Example.com/a?b=1"),
            Ok(http("example.com", 80, "/a?b=1"))
        );
        assert_eq!(
            parse_target("GET", "http://example.com"),
            Ok(http("example.com", 80, "/"))
        );
        assert_eq!(
            parse_target("GET", "http://localhost:8080?x"),
            Ok(http("localhost", 8080, "/?x"))
        );
        let tunnel = Target {
            host: "example.com".into(),
            port: 443,
            path: None,
        };
        assert_eq!(parse_target("CONNECT", "example.com:443"), Ok(tunnel));
    }

    #[test]
    fn rejects_targets_a_proxy_cannot_use() {
        for (method, target) in [
            ("GET", "/just/a/path"),          // origin-form: not a proxy request
            ("GET", "https://example.com/"),  // https must use CONNECT
            ("GET", "http://:80/"),           // no host
            ("GET", "http://example.com:x/"), // bad port
            ("GET", "http://user:pw@host/"),  // userinfo
            ("CONNECT", "example.com"),       // CONNECT needs a port
            ("CONNECT", "example.com:99999"), // port out of range
        ] {
            assert!(parse_target(method, target).is_err(), "{method} {target}");
        }
    }

    #[test]
    fn blocks_hosts_and_their_subdomains_only() {
        let config = config(&["Example.com"]);
        assert!(config.is_blocked("example.com"));
        assert!(config.is_blocked("www.example.com"));
        assert!(!config.is_blocked("notexample.com"));
        assert!(!config.is_blocked("example.org"));
    }

    #[test]
    fn rewrite_strips_hop_by_hop_headers_and_adds_via() {
        let req = request(
            "POST http://example.com:8080/submit HTTP/1.1\r\n\
             Host: wrong.example\r\n\
             User-Agent: test\r\n\
             Proxy-Connection: Keep-Alive\r\n\
             Proxy-Authorization: Basic Zm9vOmJhcg==\r\n\
             Connection: keep-alive, X-Secret\r\n\
             X-Secret: only-for-the-proxy\r\n\
             Keep-Alive: timeout=5\r\n\
             Content-Length: 3\r\n\r\n",
        );
        let (target, body_len) = plan(&req, &config(&[])).unwrap();
        assert_eq!(body_len, 3);
        let path = target.path.as_deref().unwrap();
        assert_eq!(
            rewrite_head(&req, &target, path),
            "POST /submit HTTP/1.1\r\n\
             User-Agent: test\r\n\
             Content-Length: 3\r\n\
             Host: example.com:8080\r\n\
             Via: 1.1 httpproxy\r\n\
             Connection: close\r\n\r\n"
        );
    }

    #[test]
    fn plan_refuses_with_the_right_status() {
        let status = |head: &str, blocked: &[&str]| {
            plan(&request(head), &config(blocked)).unwrap_err().status
        };
        assert_eq!(
            status("GET http://ads.example/ HTTP/1.1\r\n\r\n", &["example"]),
            403
        );
        assert_eq!(status("GET /local HTTP/1.1\r\n\r\n", &[]), 400);
        let chunked = "POST http://a/ HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert_eq!(status(chunked, &[]), 411);
        let smuggle = "POST http://a/ HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 50\r\n\r\n";
        assert_eq!(status(smuggle, &[]), 400);
    }

    #[test]
    fn status_lines_and_upstream_errors() {
        assert_eq!(parse_status(b"HTTP/1.1 404 Not Found\r\n"), Some(404));
        assert_eq!(parse_status(b"HTTP/1.0 200\n"), Some(200));
        assert_eq!(parse_status(b"SSH-2.0-OpenSSH\r\n"), None);
        assert_eq!(parse_status(b"HTTP/1.1 999 Nope\r\n"), None);
        assert_eq!(parse_status(b""), None);
        assert_eq!(upstream_error(io::ErrorKind::TimedOut.into()).status, 504);
        assert_eq!(upstream_error(io::ErrorKind::WouldBlock.into()).status, 504);
        assert_eq!(
            upstream_error(io::ErrorKind::ConnectionRefused.into()).status,
            502
        );
    }

    #[test]
    fn head_parser_rejects_bad_input_and_keeps_early_bytes() {
        let mut input = &b"CONNECT a:443 HTTP/1.1\r\n\r\n\x16\x03\x01"[..];
        assert_eq!(read_head(&mut input).unwrap().method, "CONNECT");
        assert_eq!(input, b"\x16\x03\x01"); // a TLS record start, left for the tunnel
        for bad in [
            "GET http://a/\r\n\r\n",
            "GET http://a/ HTTP/1.1\r\nNoColon\r\n\r\n",
        ] {
            assert!(matches!(
                read_head(&mut bad.as_bytes()),
                Err(HeadError::Malformed(_))
            ));
        }
        let huge = format!(
            "GET http://a/ HTTP/1.1\r\nX: {}\r\n\r\n",
            "a".repeat(MAX_HEAD)
        );
        assert!(matches!(
            read_head(&mut huge.as_bytes()),
            Err(HeadError::TooLarge)
        ));
        assert!(matches!(
            read_head(&mut &b"GET http://a/ HT"[..]),
            Err(HeadError::Io(_))
        ));
    }
}
