//! fetchr — a curl-lite HTTP/1.1 client over a plain `TcpStream`.
//!
//! # The client side of HTTP/1.1 (RFC 9112)
//!
//! `fetchr http://127.0.0.1:8180/files/hello.txt` connects to port 8180
//! and writes:
//!
//! ```text
//! GET /files/hello.txt HTTP/1.1\r\n   only path + query go on the request line (§3.2.1)
//! Host: 127.0.0.1:8180\r\n            the rest of the URL goes here; required in 1.1 (§3.2)
//! User-Agent: fetchr/0.1.0\r\n
//! Accept: */*\r\n
//! Connection: close\r\n               one request per connection keeps things simple
//! \r\n                                end of head; no body for a GET
//! ```
//!
//! Sending is easy. Reading the answer is the hard part, because the body
//! has no terminator. The response head says how the body is framed, and
//! RFC 9112 §6.3 says to check in this order:
//!
//! 1. **No body.** A response to HEAD, and any 1xx, 204, or 304, whatever
//!    the headers claim.
//! 2. **`Transfer-Encoding: chunked`.** Size-prefixed pieces until a
//!    zero-size one. See [`chunked`].
//! 3. **`Content-Length: N`.** Exactly N bytes. More would be the next
//!    response; fewer means the connection broke.
//! 4. **Neither.** The body runs until the server closes the connection
//!    (the HTTP/1.0 way).
//!
//! Each case is a different `Read` stacked on the same buffered socket:
//! `take(n)`, a [`ChunkedReader`], or the socket reader itself. `io::copy`
//! doesn't care which one it gets.
//!
//! **Redirects** (RFC 9110 §15.4). 301, 302, 303, 307, and 308 carry a
//! `Location` header. With `-L` we resolve it against the current URL and
//! try again, at most [`MAX_REDIRECTS`] times. A 303 means "GET the result
//! over there", so the method becomes GET and the body is dropped. A 307 or
//! 308 promises that method and body stay the same.

pub mod chunked;
pub mod url;

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub use chunked::ChunkedReader;
pub use url::Url;

pub const MAX_REDIRECTS: usize = 10;
const USER_AGENT: &str = concat!("fetchr/", env!("CARGO_PKG_VERSION"));
const MAX_LINE: u64 = 8 * 1024;
const MAX_HEADERS: usize = 100;
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Everything from the command line except the URL.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `-X`. Without it the method follows from `head` and `data`.
    pub method: Option<String>,
    /// `-d`: a request body, sent as a form like curl does.
    pub data: Option<String>,
    /// `-H`, in order. These replace our default header of the same name.
    pub headers: Vec<(String, String)>,
    /// `-I`: send HEAD and print the response headers.
    pub head: bool,
    /// `-v`: print `>` and `<` lines to the log.
    pub verbose: bool,
    /// `-L`: follow redirects.
    pub follow: bool,
}

impl Options {
    /// curl's rule: `-X` wins, then `-I` means HEAD, `-d` means POST.
    pub fn method(&self) -> String {
        match (&self.method, self.head, &self.data) {
            (Some(method), _, _) => method.clone(),
            (None, true, _) => "HEAD".into(),
            (None, false, Some(_)) => "POST".into(),
            (None, false, None) => "GET".into(),
        }
    }
}

/// The request head as lines, without CRLFs, so `-v` can print them too.
pub fn request_head(
    method: &str,
    url: &Url,
    user_headers: &[(String, String)],
    body: Option<&str>,
) -> Vec<String> {
    let mut lines = vec![format!("{method} {} HTTP/1.1", url.path)];
    let user_has = |name: &str| {
        user_headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    };
    let mut defaults = vec![
        ("Host", url.authority()),
        ("User-Agent", USER_AGENT.to_string()),
        ("Accept", "*/*".to_string()),
        ("Connection", "close".to_string()),
    ];
    if let Some(body) = body {
        defaults.push((
            "Content-Type",
            "application/x-www-form-urlencoded".to_string(),
        ));
        defaults.push(("Content-Length", body.len().to_string()));
    }
    for (name, value) in defaults {
        if !user_has(name) {
            lines.push(format!("{name}: {value}"));
        }
    }
    lines.extend(user_headers.iter().map(|(n, v)| format!("{n}: {v}")));
    lines
}

#[derive(Debug)]
pub struct ResponseHead {
    /// `HTTP/1.1 200 OK`, kept whole for printing.
    pub status_line: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl ResponseHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        let found = self
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name));
        found.map(|(_, v)| v.as_str())
    }
}

/// Reads the status line and headers, up to and including the empty line.
pub fn read_head(r: &mut impl BufRead) -> io::Result<ResponseHead> {
    let status_line = read_line(r)?;
    // The reason phrase may contain spaces or be missing: split at most twice.
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next().unwrap_or_default().starts_with("HTTP/1.") {
        return Err(invalid_data(format!(
            "not an HTTP/1.x response: {status_line:?}"
        )));
    }
    let status = parts
        .next()
        .filter(|code| code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| invalid_data(format!("bad status line {status_line:?}")))?;

    let mut headers = Vec::new();
    loop {
        let line = read_line(r)?;
        if line.is_empty() {
            return Ok(ResponseHead {
                status_line,
                status,
                headers,
            });
        }
        if headers.len() == MAX_HEADERS {
            return Err(invalid_data("too many response headers"));
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid_data(format!("bad header line {line:?}")))?;
        headers.push((name.to_string(), value.trim().to_string()));
    }
}

/// How the response body is delimited. See the module docs.
#[derive(Debug, PartialEq, Eq)]
pub enum Framing {
    Empty,
    Chunked,
    Length(u64),
    UntilClose,
}

/// RFC 9112 §6.3, in order of precedence.
pub fn framing(method: &str, head: &ResponseHead) -> io::Result<Framing> {
    if method == "HEAD" || head.status < 200 || head.status == 204 || head.status == 304 {
        return Ok(Framing::Empty);
    }
    if let Some(codings) = head.header("transfer-encoding") {
        // `gzip, chunked`: only the last coding frames the message. If it
        // isn't chunked, the body runs to the close.
        let last = codings.rsplit(',').next().unwrap_or_default().trim();
        let chunked = last.eq_ignore_ascii_case("chunked");
        return Ok(if chunked {
            Framing::Chunked
        } else {
            Framing::UntilClose
        });
    }
    if let Some(len) = head.header("content-length") {
        let len = len
            .parse()
            .map_err(|_| invalid_data(format!("bad Content-Length {len:?}")))?;
        return Ok(Framing::Length(len));
    }
    Ok(Framing::UntilClose)
}

/// Copies the body to `out` and returns its length.
pub fn copy_body(framing: Framing, r: &mut impl BufRead, out: &mut impl Write) -> io::Result<u64> {
    match framing {
        Framing::Empty => Ok(0),
        Framing::Chunked => io::copy(&mut ChunkedReader::new(r), out),
        Framing::Length(len) => {
            let got = io::copy(&mut r.take(len), out)?;
            if got < len {
                let msg = format!(
                    "connection closed with {} of {len} body bytes missing",
                    len - got
                );
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, msg));
            }
            Ok(got)
        }
        Framing::UntilClose => io::copy(r, out),
    }
}

pub fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Performs the request, following redirects if `-L` was given. The body
/// (or with `-I`, the headers) goes to `out` and `-v` output goes to `log`.
/// Returns the final status code.
pub fn fetch(
    url: &Url,
    opts: &Options,
    out: &mut impl Write,
    log: &mut impl Write,
) -> io::Result<u16> {
    let mut url = url.clone();
    let mut method = opts.method();
    let mut body = opts.data.clone();
    let mut redirects = 0;
    loop {
        let stream = TcpStream::connect((url.host.as_str(), url.port))?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        let lines = request_head(&method, &url, &opts.headers, body.as_deref());
        if opts.verbose {
            let ip = stream.peer_addr()?.ip();
            writeln!(log, "* Connected to {} ({ip}) port {}", url.host, url.port)?;
            for line in &lines {
                writeln!(log, "> {line}")?;
            }
            writeln!(log, ">")?;
        }
        let mut request = lines.join("\r\n") + "\r\n\r\n";
        request.push_str(body.as_deref().unwrap_or_default());
        // `&TcpStream` is a `Write` (and a `Read`), so no `mut` socket needed.
        (&stream).write_all(request.as_bytes())?;

        let mut reader = BufReader::new(&stream);
        let head = read_head(&mut reader)?;
        if opts.verbose {
            print_head(log, &head, "< ")?;
        }
        if opts.head {
            print_head(out, &head, "")?;
        }

        match head.header("location") {
            Some(location) if opts.follow && is_redirect(head.status) => {
                if redirects == MAX_REDIRECTS {
                    let msg = format!("maximum ({MAX_REDIRECTS}) redirects followed");
                    return Err(io::Error::other(msg));
                }
                redirects += 1;
                url = url.join(location).map_err(invalid_data)?;
                if head.status == 303 && method != "HEAD" {
                    method = "GET".to_string();
                    body = None;
                }
                if opts.verbose {
                    writeln!(log, "* Following redirect to {url}")?;
                }
                // We sent `Connection: close`, so this connection is finished
                // anyway. No need to read the redirect's body.
            }
            _ => {
                copy_body(framing(&method, &head)?, &mut reader, out)?;
                out.flush()?;
                return Ok(head.status);
            }
        }
    }
}

fn print_head(w: &mut impl Write, head: &ResponseHead, prefix: &str) -> io::Result<()> {
    writeln!(w, "{prefix}{}", head.status_line)?;
    for (name, value) in &head.headers {
        writeln!(w, "{prefix}{name}: {value}")?;
    }
    writeln!(w, "{}", prefix.trim_end())
}

/// One line of at most `MAX_LINE` bytes, without its CRLF (or bare LF). EOF
/// is an error here: a response never legitimately ends in the middle of a
/// line.
pub(crate) fn read_line(r: &mut impl BufRead) -> io::Result<String> {
    let mut buf = Vec::new();
    r.take(MAX_LINE).read_until(b'\n', &mut buf)?;
    if buf.last() != Some(&b'\n') {
        if buf.len() as u64 == MAX_LINE {
            return Err(invalid_data("line too long"));
        }
        let msg = "connection closed in the middle of the response";
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, msg));
    }
    buf.pop();
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    // A client should be liberal: one odd byte in a header shouldn't abort
    // the download, so replace invalid UTF-8 instead of failing.
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) fn invalid_data(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(raw: &str) -> io::Result<ResponseHead> {
        read_head(&mut raw.as_bytes())
    }

    #[test]
    fn builds_the_request_head() {
        let url: Url = "http://example.com:8080/a?b=1".parse().unwrap();
        let lines = request_head("GET", &url, &[], None);
        assert_eq!(lines[0], "GET /a?b=1 HTTP/1.1");
        assert_eq!(lines[1], "Host: example.com:8080");
        assert!(lines.contains(&"Connection: close".to_string()));

        let custom = [("user-agent".to_string(), "me/1".to_string())];
        let lines = request_head("POST", &url, &custom, Some("x=1"));
        assert!(lines.contains(&"Content-Length: 3".to_string()));
        assert!(lines.contains(&"user-agent: me/1".to_string()));
        assert!(!lines.iter().any(|l| l.starts_with("User-Agent")));
    }

    #[test]
    fn picks_the_method_like_curl() {
        let mut opts = Options::default();
        assert_eq!(opts.method(), "GET");
        opts.data = Some("x".into());
        assert_eq!(opts.method(), "POST");
        opts.head = true;
        assert_eq!(opts.method(), "HEAD");
        opts.method = Some("PUT".into());
        assert_eq!(opts.method(), "PUT");
    }

    #[test]
    fn reads_response_heads() {
        let h =
            head("HTTP/1.1 404 Not Found Here\r\nContent-Length: 0\r\nX-A:b\r\n\r\nbody").unwrap();
        assert_eq!(
            (h.status, h.status_line.as_str()),
            (404, "HTTP/1.1 404 Not Found Here")
        );
        assert_eq!(
            (h.header("content-length"), h.header("x-a")),
            (Some("0"), Some("b"))
        );
        assert_eq!(head("HTTP/1.0 204\n\n").unwrap().status, 204); // bare LF, no reason
    }

    #[test]
    fn rejects_bad_response_heads() {
        for raw in [
            "SSH-2.0-OpenSSH_9.6\r\n",
            "HTTP/1.1 2000 OK\r\n\r\n",
            "HTTP/1.1 +20 OK\r\n\r\n",
            "HTTP/1.1 200 OK\r\nno colon here\r\n\r\n",
            "HTTP/1.1 200 OK\r\nX: y\r\n", // EOF before the empty line
        ] {
            assert!(head(raw).is_err(), "{raw:?}");
        }
        let many = format!("HTTP/1.1 200 OK\r\n{}\r\n", "X: y\r\n".repeat(101));
        assert!(head(&many).is_err());
    }

    #[test]
    fn frames_bodies_in_rfc_order() {
        let f = |method: &str, raw: &str| framing(method, &head(raw).unwrap()).unwrap();
        assert_eq!(
            f("HEAD", "HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n"),
            Framing::Empty
        );
        assert_eq!(
            f(
                "GET",
                "HTTP/1.1 304 Not Modified\r\nContent-Length: 9\r\n\r\n"
            ),
            Framing::Empty
        );
        assert_eq!(
            f("GET", "HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n"),
            Framing::Length(9)
        );
        let both = "HTTP/1.1 200 OK\r\nContent-Length: 9\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert_eq!(f("GET", both), Framing::Chunked);
        assert_eq!(
            f("GET", "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n"),
            Framing::UntilClose
        );
        assert_eq!(f("GET", "HTTP/1.0 200 OK\r\n\r\n"), Framing::UntilClose);
        assert!(
            framing(
                "GET",
                &head("HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n").unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn copy_body_respects_content_length() {
        let mut out = Vec::new();
        copy_body(Framing::Length(5), &mut &b"helloEXTRA"[..], &mut out).unwrap();
        assert_eq!(out, b"hello");
        let err = copy_body(Framing::Length(10), &mut &b"abc"[..], &mut Vec::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
