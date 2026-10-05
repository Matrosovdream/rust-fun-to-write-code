//! The HTTP/1.1 message layer. [`parse_request`] reads a [`Request`] from any
//! `BufRead` and [`Response::write_to`] writes to any `Write`. There are no
//! sockets in this file, so the tests just feed in byte slices.

use std::fmt;
use std::io::{self, BufRead, Read, Write};

/// Longest request line we accept. RFC 9112 §3 recommends at least 8000.
pub const MAX_REQUEST_LINE: u64 = 8 * 1024;
/// Budget for all header lines together.
pub const MAX_HEADER_BYTES: u64 = 8 * 1024;
pub const MAX_HEADERS: usize = 100;
/// A static file server has no use for big uploads.
pub const MAX_BODY: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    Http10,
    Http11,
}

#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// The request-target exactly as sent, still percent-encoded.
    pub target: String,
    pub version: Version,
    /// In arrival order. Names keep their case; lookups ignore it.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// Header names are case-insensitive (RFC 9110 §5.1).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The target without its `?query`.
    pub fn path(&self) -> &str {
        self.target
            .split_once('?')
            .map_or(self.target.as_str(), |(path, _)| path)
    }

    /// RFC 9112 §9.3: HTTP/1.1 connections persist unless either side says
    /// `Connection: close`. HTTP/1.0 ones close (we skip 1.0's opt-in
    /// `keep-alive` extension). `Connection` holds a comma-separated list.
    pub fn keep_alive(&self) -> bool {
        let close = self.header("connection").is_some_and(|v| {
            v.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("close"))
        });
        self.version == Version::Http11 && !close
    }
}

/// Why a request couldn't be read. Every variant except `Io` becomes an
/// error response; after any of them the connection is closed, because we
/// no longer know where the next request would start.
#[derive(Debug)]
pub enum ParseError {
    /// Timeout, reset, or EOF in the middle of a request: just hang up.
    Io(io::Error),
    BadRequest(&'static str),
    UriTooLong,
    HeadersTooLarge,
    BodyTooLarge,
}

impl ParseError {
    pub fn status(&self) -> u16 {
        match self {
            ParseError::Io(_) | ParseError::BadRequest(_) => 400,
            ParseError::UriTooLong => 414,
            ParseError::HeadersTooLarge => 431,
            ParseError::BodyTooLarge => 413,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Io(e) => write!(f, "{e}"),
            ParseError::BadRequest(why) => write!(f, "{why}"),
            ParseError::UriTooLong => write!(f, "request line over {MAX_REQUEST_LINE} bytes"),
            ParseError::HeadersTooLarge => {
                write!(f, "over {MAX_HEADERS} headers or {MAX_HEADER_BYTES} bytes")
            }
            ParseError::BodyTooLarge => write!(f, "body over {MAX_BODY} bytes"),
        }
    }
}

/// Lets `?` turn an `io::Error` into a `ParseError` automatically.
impl From<io::Error> for ParseError {
    fn from(e: io::Error) -> Self {
        ParseError::Io(e)
    }
}

fn eof() -> ParseError {
    ParseError::Io(io::ErrorKind::UnexpectedEof.into())
}

/// Reads one request: the request line, the headers, then a body if
/// `Content-Length` announces one. `Ok(None)` means the peer closed the
/// connection cleanly between requests.
pub fn parse_request(r: &mut impl BufRead) -> Result<Option<Request>, ParseError> {
    let Some(line) = read_line(r, MAX_REQUEST_LINE, ParseError::UriTooLong)? else {
        return Ok(None);
    };
    let (method, target, version) = parse_request_line(&line)?;

    let mut headers = Vec::new();
    let mut budget = MAX_HEADER_BYTES;
    loop {
        let line = read_line(r, budget, ParseError::HeadersTooLarge)?.ok_or_else(eof)?;
        if line.is_empty() {
            break; // the empty line that ends the head
        }
        budget = budget.saturating_sub(line.len() as u64 + 2);
        if headers.len() == MAX_HEADERS || budget == 0 {
            return Err(ParseError::HeadersTooLarge);
        }
        // `name: value`. No space is allowed before the colon (RFC 9112
        // §5.1), and a folded continuation line starts with a space (§5.2);
        // `is_token` rejects both.
        let (name, value) = line
            .split_once(':')
            .ok_or(ParseError::BadRequest("header line without a colon"))?;
        if !is_token(name) {
            return Err(ParseError::BadRequest("invalid header name"));
        }
        let value = value.trim_matches([' ', '\t']); // OWS around the value
        headers.push((name.to_string(), value.to_string()));
    }

    let mut req = Request {
        method,
        target,
        version,
        headers,
        body: Vec::new(),
    };
    if version == Version::Http11 && req.header("host").is_none() {
        return Err(ParseError::BadRequest("missing Host header")); // RFC 9112 §3.2
    }
    // We only frame request bodies by Content-Length. If we can't decode a
    // Transfer-Encoding we can't tell where the body ends (RFC 9112 §6.3).
    if req.header("transfer-encoding").is_some() {
        return Err(ParseError::BadRequest("Transfer-Encoding not supported"));
    }
    if let Some(value) = req.header("content-length") {
        let len = parse_digits(value).ok_or(ParseError::BadRequest("bad Content-Length"))?;
        if len > MAX_BODY {
            return Err(ParseError::BodyTooLarge);
        }
        // `take(len)` turns the stream into a reader that ends after exactly
        // `len` bytes, so we never eat into the next pipelined request.
        r.take(len).read_to_end(&mut req.body)?;
        if req.body.len() as u64 != len {
            return Err(eof());
        }
    }
    Ok(Some(req))
}

fn parse_request_line(line: &str) -> Result<(String, String, Version), ParseError> {
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError::BadRequest(
            "request line is not `METHOD target VERSION`",
        ));
    };
    if !is_token(method) {
        return Err(ParseError::BadRequest("invalid method"));
    }
    // Visible ASCII only: no spaces, no control characters.
    if target.is_empty() || !target.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ParseError::BadRequest("invalid request target"));
    }
    let version = match version {
        "HTTP/1.1" => Version::Http11,
        "HTTP/1.0" => Version::Http10,
        _ => return Err(ParseError::BadRequest("unsupported HTTP version")),
    };
    Ok((method.to_string(), target.to_string(), version))
}

/// Reads one line of at most `limit` bytes and strips its ending. Bare LF
/// is accepted as well as CRLF, as RFC 9112 §2.2 allows. `Ok(None)` means
/// EOF before the first byte; a line that hits the limit returns `too_long`.
fn read_line(
    r: &mut impl BufRead,
    limit: u64,
    too_long: ParseError,
) -> Result<Option<String>, ParseError> {
    let mut buf = Vec::new();
    // Without `take`, a client sending gigabytes and no newline would make
    // `read_until` allocate gigabytes.
    let n = r.take(limit).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        return Err(if n as u64 == limit { too_long } else { eof() });
    }
    buf.pop();
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    if buf.contains(&b'\r') {
        return Err(ParseError::BadRequest("bare CR")); // RFC 9112 §2.2
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| ParseError::BadRequest("line is not UTF-8"))
}

/// A "token" (RFC 9110 §5.6.2): the characters allowed in methods and
/// header names.
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Digits only. `str::parse` alone would also accept `+5`.
fn parse_digits(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok() // still fails on overflow
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        let headers = vec![("Content-Type".to_string(), content_type.to_string())];
        Response {
            status,
            headers,
            body: body.into(),
        }
    }

    /// A tiny text page that just names the status, e.g. `404 Not Found`.
    pub fn plain(status: u16) -> Self {
        let body = format!("{status} {}\n", reason(status));
        Response::new(status, "text/plain; charset=utf-8", body)
    }

    /// Builder-style: takes `self` by value and hands it back, so calls chain.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Serializes the response. With `head_only` (a HEAD request) the body
    /// is left out, but Content-Length still describes what a GET would
    /// get (RFC 9110 §9.3.2).
    pub fn write_to(&self, w: &mut impl Write, head_only: bool) -> io::Result<()> {
        write!(w, "HTTP/1.1 {} {}\r\n", self.status, reason(self.status))?;
        for (name, value) in &self.headers {
            write!(w, "{name}: {value}\r\n")?;
        }
        write!(w, "Content-Length: {}\r\n\r\n", self.body.len())?;
        if !head_only {
            w.write_all(&self.body)?;
        }
        Ok(())
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `&[u8]` implements `BufRead`, so a byte string stands in for a socket.
    fn parse(raw: &str) -> Result<Option<Request>, ParseError> {
        parse_request(&mut raw.as_bytes())
    }

    fn status_of(raw: &str) -> u16 {
        parse(raw).expect_err(raw).status()
    }

    #[test]
    fn parses_a_well_formed_request() {
        let req = parse("GET /a%20b?x=1 HTTP/1.1\r\nHost: h\r\nX-Thing:  spaced value \r\n\r\n")
            .unwrap()
            .unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.target, "/a%20b?x=1");
        assert_eq!(req.path(), "/a%20b");
        assert_eq!(req.version, Version::Http11);
        assert_eq!(req.header("HOST"), Some("h"));
        assert_eq!(req.header("x-thing"), Some("spaced value"));
        assert!(req.keep_alive());
    }

    #[test]
    fn accepts_bare_lf() {
        let req = parse("HEAD / HTTP/1.1\nHost: h\n\n").unwrap().unwrap();
        assert_eq!(req.method, "HEAD");
        assert_eq!(req.header("host"), Some("h"));
    }

    #[test]
    fn reads_exactly_content_length_bytes_of_body() {
        let raw = "POST /a HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello\
                   GET /b HTTP/1.1\r\nHost: h\r\n\r\n";
        let mut r = raw.as_bytes();
        let first = parse_request(&mut r).unwrap().unwrap();
        assert_eq!(first.body, b"hello");
        let second = parse_request(&mut r).unwrap().unwrap();
        assert_eq!(second.target, "/b");
        assert!(parse_request(&mut r).unwrap().is_none()); // clean EOF
    }

    #[test]
    fn partial_input_is_an_io_error() {
        for raw in [
            "GET / HTTP/1.1\r\nHost: h\r\n", // head never ends
            "GET / HTTP/1.1\r\nHo",          // EOF mid-line
            "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 10\r\n\r\nabc", // short body
        ] {
            assert!(matches!(parse(raw), Err(ParseError::Io(_))), "{raw:?}");
        }
    }

    #[test]
    fn rejects_malformed_requests_with_400() {
        for raw in [
            "hello\r\n\r\n",
            "GET /\r\n\r\n",
            "GET  / HTTP/1.1\r\nHost: h\r\n\r\n",
            "GET / HTTP/2.0\r\nHost: h\r\n\r\n",
            "G(T / HTTP/1.1\r\nHost: h\r\n\r\n",
            "GET / HTTP/1.1\r\n\r\n",             // no Host
            "GET / HTTP/1.1\r\nHost h\r\n\r\n",   // no colon
            "GET / HTTP/1.1\r\nHost : h\r\n\r\n", // space before colon
            "GET / HTTP/1.1\r\nHost: h\r\n folded\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: h\rX: y\r\n\r\n", // bare CR
            "GET / HTTP/1.1\r\nHost: h\r\nContent-Length: +5\r\n\r\nhello",
            "GET / HTTP/1.1\r\nHost: h\r\nContent-Length: 99999999999999999999\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n",
        ] {
            assert_eq!(status_of(raw), 400, "{raw:?}");
        }
    }

    #[test]
    fn enforces_size_limits() {
        let long_target = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(9000));
        assert_eq!(status_of(&long_target), 414);

        let huge_header = format!(
            "GET / HTTP/1.1\r\nHost: h\r\nX: {}\r\n\r\n",
            "a".repeat(9000)
        );
        assert_eq!(status_of(&huge_header), 431);

        let many_headers = format!(
            "GET / HTTP/1.1\r\nHost: h\r\n{}\r\n",
            "X: y\r\n".repeat(101)
        );
        assert_eq!(status_of(&many_headers), 431);

        assert_eq!(
            status_of("POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 2000000\r\n\r\n"),
            413
        );
    }

    #[test]
    fn keep_alive_rules() {
        let ka = |raw: &str| parse(raw).unwrap().unwrap().keep_alive();
        assert!(ka("GET / HTTP/1.1\r\nHost: h\r\n\r\n"));
        assert!(!ka(
            "GET / HTTP/1.1\r\nHost: h\r\nConnection: close\r\n\r\n"
        ));
        assert!(!ka(
            "GET / HTTP/1.1\r\nHost: h\r\nconnection: Upgrade, CLOSE\r\n\r\n"
        ));
        assert!(!ka("GET / HTTP/1.0\r\n\r\n"));
    }

    #[test]
    fn writes_a_response_and_a_head_response() {
        let resp = Response::new(200, "text/plain", "hi").with_header("X-A", "1");
        let mut out = Vec::new();
        resp.write_to(&mut out, false).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-A: 1\r\nContent-Length: 2\r\n\r\nhi"
        );

        let mut out = Vec::new();
        resp.write_to(&mut out, true).unwrap();
        assert!(out.ends_with(b"Content-Length: 2\r\n\r\n"));
    }
}
