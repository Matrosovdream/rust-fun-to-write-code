//! binkv — a key-value store spoken over a small binary protocol.
//!
//! Text protocols (echoserver, miniredis) find the end of a message by
//! scanning for `\n`. Binary protocols usually say up front how long the
//! message is: read a fixed-size length, then read exactly that many bytes.
//! Nothing needs escaping, keys and values may hold any bytes, and the
//! reader never has to search.
//!
//! # Handshake
//!
//! Right after connecting, the client sends 4 magic bytes, `BKV1`
//! (`42 4B 56 31`). They let the server turn away anything that isn't a
//! binkv client (a browser, a stray `nc`), and the `1` is a protocol
//! version: a future `BKV2` could change the format without confusing old
//! servers. The server doesn't answer a good magic. A bad one gets a single
//! ERROR frame, then the server hangs up.
//!
//! # Frames
//!
//! After the magic, both directions carry frames. Every integer is
//! big-endian ("network byte order"):
//!
//! ```text
//! +----------------+----------+---------------------+
//! | length: u32 BE | kind: u8 | payload             |
//! +----------------+----------+---------------------+
//!   = 1 + payload size (it never counts itself); at most 1 MiB
//! ```
//!
//! `kind` is the opcode in a request and the status in a response. A
//! payload is a sequence of *strings*, each `[len: u16 BE][len bytes]`, so
//! a key or a value is at most 65535 bytes.
//!
//! | Opcode | Request | Request payload | Payload of the OK reply |
//! |--------|---------|-----------------|-------------------------|
//! | `0x01` | GET     | key             | value                   |
//! | `0x02` | PUT     | key, value      | (empty)                 |
//! | `0x03` | DEL     | key             | (empty)                 |
//! | `0x04` | LIST    | (empty)         | every key, sorted       |
//! | `0x05` | PING    | (empty)         | `"PONG"`                |
//!
//! | Status | Response  | Payload                              |
//! |--------|-----------|--------------------------------------|
//! | `0x00` | OK        | the strings above                    |
//! | `0x01` | NOT_FOUND | (empty): GET or DEL of a missing key |
//! | `0x02` | ERROR     | one string, a human-readable message |
//!
//! A PING and its reply, byte by byte:
//!
//! ```text
//! client → 42 4B 56 31                          "BKV1"
//!          00 00 00 01  05                      length 1, PING
//! server ← 00 00 00 07  00  00 04 50 4F 4E 47   length 7, OK, "PONG"
//! ```
//!
//! A response doesn't say which request it answers, and doesn't need to:
//! the client sends one request and reads one response, in order.
//!
//! # When things go wrong
//!
//! - Unknown opcode, or a payload that doesn't fit its opcode: ERROR, and
//!   the connection stays open. The length prefix told us where the bad
//!   frame ends, so the next frame can still be found.
//! - Bad magic, or a length over 1 MiB: ERROR, then close. We refuse to
//!   read an oversized frame, so we can't skip past it either.
//! - A client that stays silent longer than the idle timeout: close.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const MAGIC: [u8; 4] = *b"BKV1";
/// Largest `length` we accept, in either direction.
pub const MAX_FRAME: usize = 1024 * 1024;
/// Largest string: whatever fits in its u16 length prefix.
pub const MAX_STR: usize = u16::MAX as usize;

pub const OP_GET: u8 = 0x01;
pub const OP_PUT: u8 = 0x02;
pub const OP_DEL: u8 = 0x03;
pub const OP_LIST: u8 = 0x04;
pub const OP_PING: u8 = 0x05;

pub const STATUS_OK: u8 = 0x00;
pub const STATUS_NOT_FOUND: u8 = 0x01;
pub const STATUS_ERROR: u8 = 0x02;

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Get(Vec<u8>),
    Put(Vec<u8>, Vec<u8>),
    Del(Vec<u8>),
    List,
    Ping,
}

/// `Ok` carries zero or more strings; which ones depends on the request
/// (see the table above). Written `Response::Ok` to keep it apart from
/// `Result::Ok`.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Ok(Vec<Vec<u8>>),
    NotFound,
    Error(String),
}

#[derive(Debug, PartialEq)]
pub enum DecodeError {
    /// A length or a string promised more bytes than the frame holds.
    Truncated,
    /// The payload had bytes left over after the last expected field.
    TrailingBytes(usize),
    UnknownOpcode(u8),
    UnknownStatus(u8),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DecodeError::Truncated => f.write_str("frame ends in the middle of a field"),
            DecodeError::TrailingBytes(n) => write!(f, "{n} unexpected bytes after the payload"),
            DecodeError::UnknownOpcode(op) => write!(f, "unknown opcode 0x{op:02x}"),
            DecodeError::UnknownStatus(s) => write!(f, "unknown status 0x{s:02x}"),
        }
    }
}

// An empty impl is enough: Display + Debug are what `Error` needs. This
// lets a DecodeError travel inside an `io::Error` (see the client).
impl std::error::Error for DecodeError {}

/// Reads fields off the front of a byte slice. Every read checks the
/// remaining length first, so a lying length field becomes an error
/// instead of an out-of-bounds panic.
///
/// The `'a` says the slices we hand out borrow from the original buffer,
/// not from the cursor: they stay valid after the cursor is gone, and no
/// bytes are copied until the caller asks (`to_vec`).
struct Cursor<'a> {
    rest: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Cursor { rest: buf }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let (head, tail) = self
            .rest
            .split_at_checked(n)
            .ok_or(DecodeError::Truncated)?;
        self.rest = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    /// `[u16 BE len][len bytes]`
    fn str(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.take(2)?;
        let len = u16::from_be_bytes([len[0], len[1]]);
        self.take(len as usize)
    }

    fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    /// Being strict about leftovers catches encoder bugs early.
    fn finish(self) -> Result<(), DecodeError> {
        match self.rest.len() {
            0 => Ok(()),
            n => Err(DecodeError::TrailingBytes(n)),
        }
    }
}

/// Every frame body has the same shape: a kind byte, then strings.
/// Callers keep each string within `MAX_STR`: keys and values arrive
/// through this same u16 prefix, and the client checks its arguments.
fn encode_body(kind: u8, strings: &[&[u8]]) -> Vec<u8> {
    let mut out = vec![kind];
    for s in strings {
        let len = u16::try_from(s.len()).expect("string longer than MAX_STR");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(s);
    }
    out
}

impl Request {
    /// The frame body: opcode + payload, without the length prefix.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Request::Get(key) => encode_body(OP_GET, &[key]),
            Request::Put(key, value) => encode_body(OP_PUT, &[key, value]),
            Request::Del(key) => encode_body(OP_DEL, &[key]),
            Request::List => encode_body(OP_LIST, &[]),
            Request::Ping => encode_body(OP_PING, &[]),
        }
    }

    pub fn decode(body: &[u8]) -> Result<Request, DecodeError> {
        let mut cur = Cursor::new(body);
        let request = match cur.u8()? {
            OP_GET => Request::Get(cur.str()?.to_vec()),
            OP_PUT => {
                let key = cur.str()?.to_vec();
                let value = cur.str()?.to_vec();
                Request::Put(key, value)
            }
            OP_DEL => Request::Del(cur.str()?.to_vec()),
            OP_LIST => Request::List,
            OP_PING => Request::Ping,
            other => return Err(DecodeError::UnknownOpcode(other)),
        };
        cur.finish()?;
        Ok(request)
    }
}

/// Short form for the server log. `{:?}` quotes the key and escapes
/// control characters, so a hostile key can't forge extra log lines.
impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = String::from_utf8_lossy;
        match self {
            Request::Get(key) => write!(f, "GET {:?}", text(key)),
            Request::Put(key, value) => write!(f, "PUT {:?} ({} bytes)", text(key), value.len()),
            Request::Del(key) => write!(f, "DEL {:?}", text(key)),
            Request::List => f.write_str("LIST"),
            Request::Ping => f.write_str("PING"),
        }
    }
}

impl Response {
    /// The frame body: status + payload, without the length prefix.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Response::Ok(items) => {
                let items: Vec<&[u8]> = items.iter().map(Vec::as_slice).collect();
                encode_body(STATUS_OK, &items)
            }
            Response::NotFound => encode_body(STATUS_NOT_FOUND, &[]),
            Response::Error(message) => encode_body(STATUS_ERROR, &[message.as_bytes()]),
        }
    }

    pub fn decode(body: &[u8]) -> Result<Response, DecodeError> {
        let mut cur = Cursor::new(body);
        let response = match cur.u8()? {
            STATUS_OK => {
                // No count field: strings simply continue until the frame
                // ends. The length prefix already tells us where that is.
                let mut items = Vec::new();
                while !cur.is_empty() {
                    items.push(cur.str()?.to_vec());
                }
                Response::Ok(items)
            }
            STATUS_NOT_FOUND => Response::NotFound,
            STATUS_ERROR => Response::Error(String::from_utf8_lossy(cur.str()?).into_owned()),
            other => return Err(DecodeError::UnknownStatus(other)),
        };
        cur.finish()?;
        Ok(response)
    }
}

/// Prepends the u32 length. One buffer means one `write_all` (no tiny
/// separate packet for the prefix), and the client can hex-dump it.
pub fn frame(body: &[u8]) -> Vec<u8> {
    let len = u32::try_from(body.len()).expect("frame body over 4 GiB");
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// Reads one frame body. `Ok(None)` is a clean EOF *between* frames; EOF
/// anywhere inside a frame is an `UnexpectedEof` error, and a length over
/// `MAX_FRAME` is `InvalidData`.
pub fn read_frame(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    // Read the first byte on its own: that's the only spot where EOF is
    // a polite goodbye rather than a peer dying mid-frame.
    if r.read(&mut len[..1])? == 0 {
        return Ok(None);
    }
    r.read_exact(&mut len[1..])?;
    let len = u32::from_be_bytes(len) as usize;

    // Never trust a length that came off the wire: check it before it
    // decides how much memory we use.
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes is over the {MAX_FRAME}-byte limit"),
        ));
    }
    // Even under the limit, don't `vec![0; len]` up front: `take` +
    // `read_to_end` grows the buffer only as bytes actually arrive, so a
    // peer can't make us allocate 1 MiB by sending four bytes.
    let mut body = Vec::new();
    r.take(len as u64).read_to_end(&mut body)?;
    if body.len() < len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(Some(body))
}

/// The store itself. Keys and values are bytes, not `String`: the
/// protocol allows any bytes, so the types do too.
pub type Map = HashMap<Vec<u8>, Vec<u8>>;
pub type Db = Arc<Mutex<Map>>;

/// Runs one request against the map. No sockets and no locking, so the
/// tests can call it directly.
pub fn execute(request: Request, map: &mut Map) -> Response {
    match request {
        Request::Ping => Response::Ok(vec![b"PONG".to_vec()]),
        Request::Get(key) => match map.get(&key) {
            Some(value) => Response::Ok(vec![value.clone()]),
            None => Response::NotFound,
        },
        Request::Put(key, value) => {
            map.insert(key, value);
            Response::Ok(vec![])
        }
        Request::Del(key) => match map.remove(&key) {
            Some(_) => Response::Ok(vec![]),
            None => Response::NotFound,
        },
        Request::List => {
            let mut keys: Vec<Vec<u8>> = map.keys().cloned().collect();
            keys.sort();
            Response::Ok(keys)
        }
    }
}

/// Accept loop: one thread per connection, all sharing `db`. Connections
/// silent for `idle_timeout` are closed.
pub fn serve(listener: TcpListener, db: Db, idle_timeout: Duration) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let db = Arc::clone(&db);
                thread::spawn(move || {
                    if let Err(e) = handle(stream, &db, idle_timeout) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

fn handle(stream: TcpStream, db: &Db, idle_timeout: Duration) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    // Without a timeout, a client that connects and goes quiet would pin
    // this thread forever.
    stream.set_read_timeout(Some(idle_timeout))?;
    // BufReader turns our many small reads (1 byte, 3 bytes, a body) into
    // few syscalls.
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != MAGIC {
        let shown = String::from_utf8_lossy(&magic);
        eprintln!("[{peer}] bad magic {shown:?}, closing");
        return send(
            &mut writer,
            &Response::Error("bad magic: expected BKV1".into()),
        );
    }
    eprintln!("[{peer}] connected");

    loop {
        let body = match read_frame(&mut reader) {
            Ok(Some(body)) => body,
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                eprintln!("[{peer}] {e}, closing");
                return send(&mut writer, &Response::Error(e.to_string()));
            }
            Err(e) if is_timeout(&e) => {
                eprintln!("[{peer}] idle timeout, closing");
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let response = match Request::decode(&body) {
            Ok(request) => {
                eprintln!("[{peer}] {request}");
                execute(request, &mut db.lock().expect("db lock"))
            }
            Err(e) => {
                eprintln!("[{peer}] bad request: {e}");
                Response::Error(format!("bad request: {e}"))
            }
        };
        send(&mut writer, &response)?;
    }
    eprintln!("[{peer}] disconnected");
    Ok(())
}

/// An expired read timeout: Unix reports it as WouldBlock, Windows as
/// TimedOut.
fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn send(w: &mut impl Write, response: &Response) -> io::Result<()> {
    let mut body = response.encode();
    // Only LIST can grow past the limit (lots of keys). The limit is part
    // of the protocol, so we must respect it when sending, too.
    if body.len() > MAX_FRAME {
        body = Response::Error("response too large; too many keys to list".into()).encode();
    }
    w.write_all(&frame(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all_frames(bytes: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        // `&[u8]` implements `Read`, so a byte slice stands in for a socket.
        let mut input = bytes;
        let mut frames = Vec::new();
        while let Some(body) = read_frame(&mut input)? {
            frames.push(body);
        }
        Ok(frames)
    }

    #[test]
    fn requests_round_trip() {
        let binary = vec![0, 0xff, b'\n'];
        for request in [
            Request::Ping,
            Request::List,
            Request::Get(b"name".to_vec()),
            Request::Put(b"name".to_vec(), b"rust".to_vec()),
            Request::Put(vec![], binary.clone()),
            Request::Del(binary),
        ] {
            assert_eq!(Request::decode(&request.encode()), Ok(request));
        }
    }

    #[test]
    fn responses_round_trip() {
        for response in [
            Response::Ok(vec![]),
            Response::Ok(vec![b"PONG".to_vec()]),
            Response::Ok(vec![b"a".to_vec(), vec![], vec![0, 0xff]]),
            Response::NotFound,
            Response::Error("nope".into()),
        ] {
            assert_eq!(Response::decode(&response.encode()), Ok(response));
        }
    }

    #[test]
    fn ping_bytes_match_the_docs() {
        assert_eq!(frame(&Request::Ping.encode()), [0, 0, 0, 1, 0x05]);
        let pong = execute(Request::Ping, &mut Map::new());
        assert_eq!(frame(&pong.encode()), *b"\0\0\0\x07\0\0\x04PONG");
        let put = Request::Put(b"k".to_vec(), b"vv".to_vec());
        assert_eq!(put.encode(), [0x02, 0, 1, b'k', 0, 2, b'v', b'v']);
    }

    #[test]
    fn malformed_payloads_are_errors_not_panics() {
        use DecodeError::*;
        let cases: &[(&[u8], DecodeError)] = &[
            (b"", Truncated),
            (&[0x09], UnknownOpcode(0x09)),
            (&[OP_GET], Truncated),                    // no length at all
            (&[OP_GET, 0], Truncated),                 // half a length
            (&[OP_GET, 0, 10, b'a', b'b'], Truncated), // says 10, has 2
            (&[OP_PUT, 0, 1, b'k'], Truncated),        // value missing
            (&[OP_PING, 0xaa], TrailingBytes(1)),      // PING has no payload
            (&[OP_DEL, 0, 1, b'k', 0, 0], TrailingBytes(2)),
        ];
        for (body, want) in cases {
            assert_eq!(Request::decode(body).as_ref(), Err(want), "body {body:?}");
        }
        assert_eq!(Response::decode(&[0x07]), Err(UnknownStatus(0x07)));
        assert_eq!(Response::decode(&[STATUS_OK, 0, 5, b'x']), Err(Truncated));
        assert_eq!(Response::decode(&[STATUS_ERROR]), Err(Truncated));
    }

    #[test]
    fn read_frame_splits_a_stream_into_frames() {
        let mut stream = frame(&Request::Ping.encode());
        stream.extend(frame(&Request::Get(b"x".to_vec()).encode()));
        stream.extend(frame(&[])); // a zero-length frame is still a frame
        let frames = read_all_frames(&stream).unwrap();
        assert_eq!(
            frames,
            vec![vec![OP_PING], vec![OP_GET, 0, 1, b'x'], vec![]]
        );
    }

    #[test]
    fn read_frame_reports_truncation() {
        let whole = frame(&Request::Get(b"name".to_vec()).encode());
        // Cut inside the length prefix and inside the body.
        for cut in [2, 6, whole.len() - 1] {
            let err = read_all_frames(&whole[..cut]).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
    }

    #[test]
    fn oversized_length_is_rejected_before_reading_the_body() {
        // No body follows. If read_frame tried to read it, we'd get
        // UnexpectedEof; InvalidData proves it stopped at the length.
        let err = read_all_frames(&[0xff, 0xff, 0xff, 0xff]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let at_limit = frame(&vec![0u8; MAX_FRAME]);
        assert_eq!(read_all_frames(&at_limit).unwrap()[0].len(), MAX_FRAME);
        let over = (MAX_FRAME as u32 + 1).to_be_bytes();
        let err = read_all_frames(&over).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn execute_covers_every_request() {
        let b = |s: &str| s.as_bytes().to_vec();
        let steps = [
            (Request::Get(b("name")), Response::NotFound),
            (Request::Del(b("name")), Response::NotFound),
            (Request::Put(b("name"), b("rust")), Response::Ok(vec![])),
            (Request::Put(b("a"), b("")), Response::Ok(vec![])),
            (Request::Get(b("name")), Response::Ok(vec![b("rust")])),
            (Request::List, Response::Ok(vec![b("a"), b("name")])),
            (Request::Del(b("name")), Response::Ok(vec![])),
            (Request::Get(b("name")), Response::NotFound),
        ];
        let mut map = Map::new();
        for (request, want) in steps {
            assert_eq!(execute(request.clone(), &mut map), want, "{request}");
        }
    }
}
