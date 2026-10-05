//! The opening handshake (RFC 6455 §4): an ordinary HTTP/1.1 GET asking to
//! upgrade, answered with `101 Switching Protocols`. After the blank line
//! that ends the 101, both sides speak frames on the same TCP connection.

use std::io::{self, BufRead, Read};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha1::{Digest, Sha1};

/// A fixed GUID from the RFC (§1.3). Mixing it into the accept key proves
/// the server understood the WebSocket request. A plain HTTP server that
/// happens to echo headers back could never produce the right answer.
pub const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// Most bytes we accept for the request (or response) line plus headers.
pub const MAX_HEAD: usize = 8 * 1024;

/// `Sec-WebSocket-Accept = base64(sha1(key + GUID))` (§4.2.2).
pub fn accept_key(key: &str) -> String {
    let mut sha = Sha1::new();
    sha.update(key.as_bytes());
    sha.update(GUID.as_bytes());
    BASE64.encode(sha.finalize())
}

/// An HTTP head: the request line (or status line) plus headers. The client
/// uses the same parser to read the server's 101.
#[derive(Debug)]
pub struct Head {
    pub first_line: String,
    pub headers: Vec<(String, String)>,
}

impl Head {
    /// Header names are case-insensitive (RFC 9110 §5.1).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// True if a comma-separated header contains `token`. Browsers send
    /// things like `Connection: keep-alive, Upgrade`.
    fn has_token(&self, name: &str, token: &str) -> bool {
        self.header(name).is_some_and(|value| {
            value
                .split(',')
                .any(|t| t.trim().eq_ignore_ascii_case(token))
        })
    }
}

#[derive(Debug)]
pub enum HeadError {
    /// The socket failed, timed out, or closed early.
    Io(io::Error),
    TooLarge,
    Malformed(&'static str),
}

impl From<io::Error> for HeadError {
    fn from(e: io::Error) -> Self {
        HeadError::Io(e)
    }
}

/// Reads lines up to the blank line that ends an HTTP head (RFC 9112 §2.1).
/// Bytes after it (the first frames!) stay in `reader`, so keep reading
/// frames from the same `BufReader`, not from the raw socket.
pub fn read_head(reader: &mut impl BufRead) -> Result<Head, HeadError> {
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
    let first_line = lines.next().ok_or(HeadError::Malformed("empty head"))?;
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
    Ok(Head {
        first_line,
        headers,
    })
}

/// Why an upgrade request was refused: an HTTP status line and a reason.
#[derive(Debug, PartialEq)]
pub struct Rejection(pub &'static str, pub &'static str);

impl Rejection {
    pub fn response(&self) -> String {
        let Rejection(status, reason) = self;
        // §4.4: on a version mismatch, list the versions we do speak.
        format!(
            "HTTP/1.1 {status}\r\nSec-WebSocket-Version: 13\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reason}\n",
            reason.len() + 1
        )
    }
}

/// Validates an upgrade request (§4.2.1) and returns the client's key.
pub fn check_upgrade(head: &Head) -> Result<&str, Rejection> {
    let bad = |reason| Err(Rejection("400 Bad Request", reason));
    let parts: Vec<&str> = head.first_line.split(' ').collect();
    if !matches!(parts[..], ["GET", _, "HTTP/1.1"]) {
        return bad("a WebSocket upgrade is GET ... HTTP/1.1");
    }
    if head.header("host").is_none() {
        return bad("missing Host header");
    }
    if !head.has_token("upgrade", "websocket") || !head.has_token("connection", "upgrade") {
        return bad("expected Upgrade: websocket and Connection: Upgrade");
    }
    if head.header("sec-websocket-version") != Some("13") {
        return Err(Rejection(
            "426 Upgrade Required",
            "only version 13 is supported",
        ));
    }
    // The key is 16 random bytes in base64 (§4.1). We never use the bytes;
    // decoding just checks the client followed the rules.
    match head.header("sec-websocket-key") {
        Some(key) if BASE64.decode(key).is_ok_and(|bytes| bytes.len() == 16) => Ok(key),
        _ => bad("Sec-WebSocket-Key must be 16 bytes in base64"),
    }
}

/// The server's answer to a valid upgrade request.
pub fn accept_response(key: &str) -> String {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    )
}

/// The client's upgrade request for `ws://{host}/ws`.
pub fn client_request(host: &str, key: &str) -> String {
    format!(
        "GET /ws HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
}

/// Checks the server's reply on the client side: it must be a 101 carrying
/// the accept key that matches the key we sent.
pub fn check_response(head: &Head, key: &str) -> Result<(), String> {
    if !head.first_line.starts_with("HTTP/1.1 101 ") {
        return Err(format!("server refused the upgrade: {}", head.first_line));
    }
    match head.header("sec-websocket-accept") {
        Some(accept) if accept == accept_key(key) => Ok(()),
        _ => Err("missing or wrong Sec-WebSocket-Accept".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sample key from RFC 6455 §1.3.
    const SAMPLE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

    fn head(text: &str) -> Head {
        read_head(&mut text.as_bytes()).unwrap()
    }

    fn upgrade_request(extra: &str) -> String {
        format!("GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\n{extra}\r\n")
    }

    #[test]
    fn rfc_sample_key() {
        assert_eq!(accept_key(SAMPLE_KEY), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn accepts_a_browser_style_upgrade() {
        // Firefox sends "keep-alive, Upgrade"; header names in any case.
        let request = upgrade_request(&format!(
            "connection: keep-alive, Upgrade\r\nsec-websocket-version: 13\r\nSec-WebSocket-Key: {SAMPLE_KEY}\r\n"
        ));
        assert_eq!(check_upgrade(&head(&request)), Ok(SAMPLE_KEY));
    }

    #[test]
    fn refuses_bad_upgrades() {
        let key = format!("Sec-WebSocket-Key: {SAMPLE_KEY}\r\n");
        let cases = [
            (
                format!("Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n{key}"),
                "ok",
            ),
            (format!("Sec-WebSocket-Version: 13\r\n{key}"), "400"), // no Connection
            (
                format!("Connection: Upgrade\r\nSec-WebSocket-Version: 8\r\n{key}"),
                "426",
            ),
            (
                "Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n".into(),
                "400",
            ), // no key
            (
                // "c2hvcnQ=" is base64 for "short": valid base64, but not 16 bytes
                "Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: c2hvcnQ=\r\n".into(),
                "400",
            ),
        ];
        for (extra, expected) in cases {
            let request = head(&upgrade_request(&extra));
            let got = match check_upgrade(&request) {
                Ok(_) => "ok",
                Err(Rejection(status, _)) => &status[..3],
            };
            assert_eq!(got, expected, "{extra:?}");
        }
        let post = head(&upgrade_request("").replacen("GET", "POST", 1));
        assert_eq!(check_upgrade(&post).unwrap_err().0, "400 Bad Request");
    }

    #[test]
    fn client_and_server_agree() {
        let request = head(&client_request("127.0.0.1:8183", SAMPLE_KEY));
        let key = check_upgrade(&request).unwrap();
        let response = head(&accept_response(key));
        assert_eq!(check_response(&response, SAMPLE_KEY), Ok(()));
        assert!(check_response(&response, "AAAAAAAAAAAAAAAAAAAAAA==").is_err());
    }

    #[test]
    fn head_parser_handles_partial_malformed_and_huge_input() {
        let mut input = &b"GET / HTTP/1.1\nHost: a\n\n\x81\x00"[..];
        assert_eq!(read_head(&mut input).unwrap().header("HOST"), Some("a"));
        assert_eq!(input, b"\x81\x00"); // the frame after the head is left unread
        assert!(matches!(
            read_head(&mut &b"GET / HTTP/1.1\r\nHost"[..]),
            Err(HeadError::Io(_))
        ));
        assert!(matches!(
            read_head(&mut &b"GET / HTTP/1.1\r\nNo colon\r\n\r\n"[..]),
            Err(HeadError::Malformed(_))
        ));
        let huge = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert!(matches!(
            read_head(&mut huge.as_bytes()),
            Err(HeadError::TooLarge)
        ));
    }
}
