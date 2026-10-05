//! Just enough HTTP/1.1 for a JSON API: parse a [`Request`] from any
//! `BufRead`, write a [`Response`] to any `Write`. (httpd has its own, fuller
//! version. Writing a second one is part of the exercise.)

use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::net::SocketAddr;

use serde::Serialize;

pub const MAX_LINE: u64 = 8 * 1024;
pub const MAX_HEAD: u64 = 16 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const MAX_BODY: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// The path without the query string, e.g. `/todos/1`.
    pub path: String,
    /// `?done=true&x=1`, split on `&` and `=`. Not percent-decoded: this
    /// API only uses plain words.
    pub query: HashMap<String, String>,
    /// Filled in by the router from `:name` segments of the route pattern.
    pub params: HashMap<String, String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub peer: SocketAddr,
    /// True if the connection should close after this request.
    pub close: bool,
}

impl Request {
    /// A bare request for `target` (path plus optional query), from
    /// 127.0.0.1. The parser starts from this, and so do tests.
    pub fn new(method: &str, target: &str) -> Request {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let query = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (k.to_string(), v.to_string())
            })
            .collect();
        Request {
            method: method.to_string(),
            path: path.to_string(),
            query,
            params: HashMap::new(),
            headers: Vec::new(),
            body: Vec::new(),
            peer: SocketAddr::from(([127, 0, 0, 1], 0)),
            close: false,
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let found = self
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name));
        found.map(|(_, v)| v.as_str())
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }
}

#[derive(Debug)]
pub enum ParseError {
    /// Timeout, reset, or EOF mid-request: hang up without a reply.
    Io(io::Error),
    /// Reply with this status and message, then close.
    Reject(u16, &'static str),
}

impl From<io::Error> for ParseError {
    fn from(e: io::Error) -> Self {
        ParseError::Io(e)
    }
}

fn reject<T>(status: u16, why: &'static str) -> Result<T, ParseError> {
    Err(ParseError::Reject(status, why))
}

fn eof() -> ParseError {
    ParseError::Io(io::ErrorKind::UnexpectedEof.into())
}

/// Reads one request. `Ok(None)` means a clean close between requests.
pub fn parse_request(
    r: &mut impl BufRead,
    peer: SocketAddr,
) -> Result<Option<Request>, ParseError> {
    let Some(line) = read_line(r, MAX_LINE, 414)? else {
        return Ok(None);
    };
    let parts: Vec<&str> = line.split(' ').collect();
    let [method, target, version] = parts[..] else {
        return reject(400, "malformed request line");
    };
    if method.is_empty()
        || !method.bytes().all(|b| b.is_ascii_uppercase())
        || !target.starts_with('/')
    {
        return reject(400, "malformed request line");
    }
    let mut req = Request::new(method, target);
    req.peer = peer;
    req.close = match version {
        "HTTP/1.1" => false,
        "HTTP/1.0" => true, // 1.0 closes unless it opts in; we don't support the opt-in
        _ => return reject(505, "only HTTP/1.0 and HTTP/1.1 are supported"),
    };

    let mut budget = MAX_HEAD;
    loop {
        let line = read_line(r, budget, 431)?.ok_or_else(eof)?;
        if line.is_empty() {
            break;
        }
        budget = budget.saturating_sub(line.len() as u64 + 2);
        if req.headers.len() == MAX_HEADERS || budget == 0 {
            return reject(431, "too many headers");
        }
        let Some((name, value)) = line.split_once(':') else {
            return reject(400, "malformed header line");
        };
        if name.is_empty() || name.contains([' ', '\t']) {
            return reject(400, "malformed header name");
        }
        req.headers
            .push((name.to_string(), value.trim().to_string()));
    }
    if req
        .header("connection")
        .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")))
    {
        req.close = true;
    }

    if req.header("transfer-encoding").is_some() {
        return reject(400, "chunked request bodies are not supported");
    }
    if let Some(value) = req.header("content-length") {
        // Digits only: `parse` alone would also accept "+5".
        let len = match value.parse::<u64>() {
            Ok(len) if value.bytes().all(|b| b.is_ascii_digit()) => len,
            _ => return reject(400, "bad Content-Length"),
        };
        if len > MAX_BODY {
            return reject(413, "body too large");
        }
        // `take` stops exactly at the end of this body, never reading into
        // the next request on the same connection.
        r.take(len).read_to_end(&mut req.body)?;
        if req.body.len() as u64 != len {
            return Err(eof());
        }
    }
    Ok(Some(req))
}

/// One line, at most `limit` bytes, without its CRLF or bare LF.
/// `Ok(None)` = EOF before any byte. Too long = `Reject(too_long, …)`.
fn read_line(
    r: &mut impl BufRead,
    limit: u64,
    too_long: u16,
) -> Result<Option<String>, ParseError> {
    let mut buf = Vec::new();
    let n = r.take(limit).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        if n as u64 == limit {
            return reject(too_long, "line too long");
        }
        return Err(eof());
    }
    buf.pop();
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    match String::from_utf8(buf) {
        Ok(line) => Ok(Some(line)),
        Err(_) => reject(400, "request is not UTF-8"),
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// An empty response, e.g. `Response::new(204)`.
    pub fn new(status: u16) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Any `Serialize` value as a JSON body. `&impl Serialize` accepts our
    /// own structs, a `Vec` of them, or a `serde_json::json!` value.
    pub fn json(status: u16, value: &impl Serialize) -> Response {
        let mut body = serde_json::to_vec(value).expect("our types always serialize");
        body.push(b'\n'); // so curl's output ends on its own line
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        Response {
            status,
            headers,
            body,
        }
    }

    /// `{"error":"…"}` with the given status.
    pub fn error(status: u16, message: &str) -> Response {
        Response::json(status, &serde_json::json!({ "error": message }))
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let found = self
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name));
        found.map(|(_, v)| v.as_str())
    }

    pub fn write_to(&self, w: &mut impl Write) -> io::Result<()> {
        write!(w, "HTTP/1.1 {} {}\r\n", self.status, reason(self.status))?;
        for (name, value) in &self.headers {
            write!(w, "{name}: {value}\r\n")?;
        }
        // A 204 must not carry Content-Length (RFC 9110 §8.6): it has no body.
        if self.status != 204 {
            write!(w, "Content-Length: {}\r\n", self.body.len())?;
        }
        w.write_all(b"\r\n")?;
        w.write_all(&self.body)
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        505 => "HTTP Version Not Supported",
        _ => "Internal Server Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Result<Option<Request>, ParseError> {
        parse_request(&mut raw.as_bytes(), SocketAddr::from(([10, 0, 0, 7], 5555)))
    }

    fn rejected(raw: &str) -> u16 {
        match parse(raw) {
            Err(ParseError::Reject(status, _)) => status,
            other => panic!("{raw:?} gave {other:?}"),
        }
    }

    #[test]
    fn parses_method_path_query_headers_and_body() {
        let raw = "POST /todos?done=true&x HTTP/1.1\r\nHost: h\r\ncontent-length: 2\r\n\r\n{}";
        let req = parse(raw).unwrap().unwrap();
        assert_eq!((req.method.as_str(), req.path.as_str()), ("POST", "/todos"));
        assert_eq!(req.query.get("done").map(String::as_str), Some("true"));
        assert_eq!(req.query.get("x").map(String::as_str), Some(""));
        assert_eq!(req.header("Content-Length"), Some("2"));
        assert_eq!((req.body.as_slice(), req.close), (&b"{}"[..], false));
        assert_eq!(req.peer.to_string(), "10.0.0.7:5555");
    }

    #[test]
    fn knows_when_to_close() {
        let close = |raw: &str| parse(raw).unwrap().unwrap().close;
        assert!(!close("GET / HTTP/1.1\r\n\r\n"));
        assert!(close("GET / HTTP/1.1\r\nConnection: close\r\n\r\n"));
        assert!(close(
            "GET / HTTP/1.1\r\nConnection: keep-alive, Close\r\n\r\n"
        ));
        assert!(close("GET / HTTP/1.0\n\n"));
        assert!(parse("").unwrap().is_none());
    }

    #[test]
    fn partial_requests_are_io_errors() {
        for raw in [
            "GET / HTTP/1.1\r\nHost: h\r\n",
            "GET / HT",
            "PUT / HTTP/1.1\r\nContent-Length: 9\r\n\r\nabc",
        ] {
            assert!(matches!(parse(raw), Err(ParseError::Io(_))), "{raw:?}");
        }
    }

    #[test]
    fn rejects_bad_and_oversized_requests() {
        assert_eq!(rejected("GET /\r\n\r\n"), 400);
        assert_eq!(rejected("get / HTTP/1.1\r\n\r\n"), 400);
        assert_eq!(rejected("GET / HTTP/2\r\n\r\n"), 505);
        assert_eq!(rejected("GET / HTTP/1.1\r\nNoColon\r\n\r\n"), 400);
        assert_eq!(rejected("GET / HTTP/1.1\r\nBad Name: x\r\n\r\n"), 400);
        assert_eq!(
            rejected("GET / HTTP/1.1\r\nContent-Length: -1\r\n\r\n"),
            400
        );
        assert_eq!(
            rejected("GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            400
        );
        assert_eq!(
            rejected(&format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(9000))),
            414
        );
        assert_eq!(
            rejected(&format!("GET / HTTP/1.1\r\n{}\r\n", "X: y\r\n".repeat(65))),
            431
        );
        let big_head = format!(
            "GET / HTTP/1.1\r\n{}\r\n",
            format!("X: {}\r\n", "a".repeat(8000)).repeat(3)
        );
        assert_eq!(rejected(&big_head), 431);
        assert_eq!(
            rejected("POST / HTTP/1.1\r\nContent-Length: 999999\r\n\r\n"),
            413
        );
    }

    #[test]
    fn writes_json_and_empty_responses() {
        let mut out = Vec::new();
        Response::error(404, "not found")
            .write_to(&mut out)
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text,
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 22\r\n\r\n{\"error\":\"not found\"}\n"
        );

        let mut out = Vec::new();
        Response::new(204).write_to(&mut out).unwrap();
        assert_eq!(out, b"HTTP/1.1 204 No Content\r\n\r\n");
    }
}
