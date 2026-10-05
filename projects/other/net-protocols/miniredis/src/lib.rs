//! miniredis — a Redis-compatible key-value server that speaks RESP2.
//!
//! # Protocol: RESP2
//!
//! Every value on the wire starts with a type byte and ends with CRLF:
//!
//! ```text
//! +OK\r\n                          simple string  (short status text)
//! -ERR unknown command\r\n         error
//! :42\r\n                          integer
//! $5\r\nhello\r\n                  bulk string    (length first, so it's binary-safe)
//! $-1\r\n                          null bulk      ("no such key")
//! *2\r\n:1\r\n:2\r\n               array          (a count, then that many values)
//! ```
//!
//! A request is an array of bulk strings, command name first. `SET a 1`,
//! then `GET a`:
//!
//! ```text
//! C: *3\r\n$3\r\nSET\r\n$1\r\na\r\n$1\r\n1\r\n
//! S: +OK\r\n
//! C: *2\r\n$3\r\nGET\r\n$1\r\na\r\n
//! S: $1\r\n1\r\n
//! ```
//!
//! For humans with `nc`, a line that doesn't start with a type byte is an
//! *inline command*: `GET a\r\n` means the same as the array above. A bare
//! `\n` works too. Clients may *pipeline*, sending many requests without
//! waiting for replies. The server answers every complete request it has
//! buffered, in order.
//!
//! Commands: `PING [msg]`, `ECHO msg`, `SET key value [EX s | PX ms]`,
//! `GET key`, `DEL key…`, `EXISTS key…`, `INCR key`, `EXPIRE key s`,
//! `TTL key`, `KEYS pattern` (with `*` and `?`), `DBSIZE`, `FLUSHALL`.
//!
//! # Design
//!
//! - [`parse`] is a streaming parser over a byte buffer. `Ok(None)` means
//!   "not enough bytes yet", so a partial read is a normal case, not an error.
//! - [`Command::from_frame`] turns a request frame into a typed command.
//! - [`Db::execute`] runs a command against a `HashMap`. It takes the current
//!   time as an argument, so tests can check expiry without sleeping. Expired
//!   keys are deleted lazily, the next time someone looks at them.
//! - [`serve`] shares one `Db` between connection threads as `Arc<Mutex<Db>>`.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Largest bulk string we accept. It's checked on the declared length,
/// before we wait for (or buffer) the bytes themselves.
pub const MAX_BULK: usize = 512 * 1024;
/// Most bytes of one unfinished request we'll hold in a connection's buffer.
pub const MAX_BUFFER: usize = 1024 * 1024;
/// Parsing is recursive, so `*1\r\n*1\r\n*1\r\n…` could overflow the stack
/// without a nesting limit.
const MAX_DEPTH: usize = 32;

#[derive(Debug, PartialEq)]
pub enum Frame {
    Simple(String),
    Error(String),
    Integer(i64),
    Bulk(Vec<u8>),
    /// `$-1`. The null array `*-1` is parsed into this too.
    Null,
    Array(Vec<Frame>),
}

/// The bytes can never become a valid frame. The text says why.
#[derive(Debug, PartialEq)]
pub struct ProtocolError(pub &'static str);

/// Parses one frame from the front of `buf`:
///
/// - `Ok(Some((frame, n)))`: a complete frame made of the first `n` bytes;
/// - `Ok(None)`: incomplete, so read more and call again;
/// - `Err(_)`: garbage. We can't tell where the next frame starts, so the
///   caller should give up on the connection.
pub fn parse(buf: &[u8]) -> Result<Option<(Frame, usize)>, ProtocolError> {
    parse_frame(buf, 0)
}

fn parse_frame(buf: &[u8], depth: usize) -> Result<Option<(Frame, usize)>, ProtocolError> {
    let Some(newline) = buf.iter().position(|&b| b == b'\n') else {
        return Ok(None);
    };
    // The header line, without its `\r\n` (or a bare `\n`).
    let line = &buf[..newline];
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let after = newline + 1;
    let rest = line.get(1..).unwrap_or_default();
    let frame = match line.first() {
        Some(b'+') => Frame::Simple(String::from_utf8_lossy(rest).into_owned()),
        Some(b'-') => Frame::Error(String::from_utf8_lossy(rest).into_owned()),
        Some(b':') => Frame::Integer(int(rest)?),
        Some(b'$') => return parse_bulk(buf, after, int(rest)?),
        Some(b'*') => return parse_array(buf, after, int(rest)?, depth),
        // No type byte: an inline command typed by a human, like `SET a 1`.
        _ if depth == 0 => {
            let words = line
                .split(u8::is_ascii_whitespace)
                .filter(|w| !w.is_empty());
            Frame::Array(words.map(|w| Frame::Bulk(w.to_vec())).collect())
        }
        _ => return Err(ProtocolError("unexpected byte inside an array")),
    };
    Ok(Some((frame, after)))
}

fn parse_bulk(buf: &[u8], start: usize, len: i64) -> Result<Option<(Frame, usize)>, ProtocolError> {
    if len == -1 {
        return Ok(Some((Frame::Null, start)));
    }
    // Never trust a length that came off the wire: check it before using it.
    let len = usize::try_from(len)
        .ok()
        .filter(|&n| n <= MAX_BULK)
        .ok_or(ProtocolError("invalid bulk length"))?;
    let end = start + len;
    if buf.len() < end + 2 {
        return Ok(None);
    }
    if &buf[end..end + 2] != b"\r\n" {
        return Err(ProtocolError("bulk string not followed by CRLF"));
    }
    Ok(Some((Frame::Bulk(buf[start..end].to_vec()), end + 2)))
}

fn parse_array(
    buf: &[u8],
    start: usize,
    len: i64,
    depth: usize,
) -> Result<Option<(Frame, usize)>, ProtocolError> {
    if len == -1 {
        return Ok(Some((Frame::Null, start)));
    }
    if len < 0 {
        return Err(ProtocolError("invalid array length"));
    }
    if depth >= MAX_DEPTH {
        return Err(ProtocolError("arrays nested too deeply"));
    }
    // Not `with_capacity(len)`: len came off the wire. Items only get pushed
    // once their bytes are actually here.
    let mut items = Vec::new();
    let mut pos = start;
    for _ in 0..len {
        let Some((item, used)) = parse_frame(&buf[pos..], depth + 1)? else {
            return Ok(None);
        };
        items.push(item);
        pos += used;
    }
    Ok(Some((Frame::Array(items), pos)))
}

fn int(digits: &[u8]) -> Result<i64, ProtocolError> {
    std::str::from_utf8(digits)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(ProtocolError("invalid integer"))
}

impl Frame {
    /// Appends the wire form of this frame to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Frame::Simple(s) => out.extend_from_slice(format!("+{}\r\n", one_line(s)).as_bytes()),
            Frame::Error(s) => out.extend_from_slice(format!("-{}\r\n", one_line(s)).as_bytes()),
            Frame::Integer(n) => out.extend_from_slice(format!(":{n}\r\n").as_bytes()),
            Frame::Bulk(bytes) => {
                out.extend_from_slice(format!("${}\r\n", bytes.len()).as_bytes());
                out.extend_from_slice(bytes);
                out.extend_from_slice(b"\r\n");
            }
            Frame::Null => out.extend_from_slice(b"$-1\r\n"),
            Frame::Array(items) => {
                out.extend_from_slice(format!("*{}\r\n", items.len()).as_bytes());
                for item in items {
                    item.encode(out);
                }
            }
        }
    }
}

/// Simple strings and errors end at the first CRLF, so a CR or LF inside one
/// (say, a command name the client sent) would break the framing.
fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Ping(Option<Vec<u8>>),
    Echo(Vec<u8>),
    Set(Vec<u8>, Vec<u8>, Option<Duration>),
    Get(Vec<u8>),
    Del(Vec<Vec<u8>>),
    Exists(Vec<Vec<u8>>),
    Incr(Vec<u8>),
    Expire(Vec<u8>, i64),
    Ttl(Vec<u8>),
    Keys(Vec<u8>),
    DbSize,
    FlushAll,
}

/// Every command name, for telling "wrong arity" apart from "unknown command".
const KNOWN: &str = "PING ECHO SET GET DEL EXISTS INCR EXPIRE TTL KEYS DBSIZE FLUSHALL";
const SYNTAX: &str = "ERR syntax error";
const NOT_INT: &str = "ERR value is not an integer or out of range";

impl Command {
    /// Builds a command from a request: an array of bulk strings, name first.
    /// `Err` holds the text of the error reply.
    pub fn from_frame(frame: Frame) -> Result<Command, String> {
        let Frame::Array(items) = frame else {
            return Err("ERR expected an array of bulk strings".into());
        };
        let mut args = Vec::new();
        for item in items {
            let Frame::Bulk(bytes) = item else {
                return Err("ERR expected an array of bulk strings".into());
            };
            args.push(bytes);
        }
        let Some((name, rest)) = args.split_first() else {
            return Err("ERR empty command".into());
        };
        let name = String::from_utf8_lossy(name).to_ascii_uppercase();
        // Slice patterns check the argument count and bind the arguments in one go.
        // (The clones are tiny next to the network round trip.)
        let cmd = match (name.as_str(), rest) {
            ("PING", []) => Command::Ping(None),
            ("PING", [msg]) => Command::Ping(Some(msg.clone())),
            ("ECHO", [msg]) => Command::Echo(msg.clone()),
            ("SET", [key, value, opts @ ..]) => {
                Command::Set(key.clone(), value.clone(), ttl_option(opts)?)
            }
            ("GET", [key]) => Command::Get(key.clone()),
            ("DEL", keys @ [_, ..]) => Command::Del(keys.to_vec()),
            ("EXISTS", keys @ [_, ..]) => Command::Exists(keys.to_vec()),
            ("INCR", [key]) => Command::Incr(key.clone()),
            ("EXPIRE", [key, secs]) => {
                Command::Expire(key.clone(), parse_int(secs).ok_or(NOT_INT)?)
            }
            ("TTL", [key]) => Command::Ttl(key.clone()),
            ("KEYS", [pattern]) => Command::Keys(pattern.clone()),
            ("DBSIZE", []) => Command::DbSize,
            ("FLUSHALL", []) => Command::FlushAll,
            (name, _) if KNOWN.split(' ').any(|known| known == name) => {
                let name = name.to_lowercase();
                return Err(format!(
                    "ERR wrong number of arguments for '{name}' command"
                ));
            }
            (name, _) => return Err(format!("ERR unknown command '{name}'")),
        };
        Ok(cmd)
    }
}

/// The optional `EX seconds` / `PX milliseconds` tail of SET.
fn ttl_option(opts: &[Vec<u8>]) -> Result<Option<Duration>, String> {
    let [unit, amount] = opts else {
        return if opts.is_empty() {
            Ok(None)
        } else {
            Err(SYNTAX.into())
        };
    };
    let amount = parse_int(amount).ok_or(NOT_INT)?;
    let amount = u64::try_from(amount)
        .ok()
        .filter(|&n| n > 0)
        .ok_or("ERR invalid expire time in 'set' command")?;
    match unit.to_ascii_uppercase().as_slice() {
        b"EX" => Ok(Some(Duration::from_secs(amount))),
        b"PX" => Ok(Some(Duration::from_millis(amount))),
        _ => Err(SYNTAX.into()),
    }
}

fn parse_int(bytes: &[u8]) -> Option<i64> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

struct Entry {
    value: Vec<u8>,
    expires: Option<Instant>,
}

impl Entry {
    fn expired(&self, now: Instant) -> bool {
        self.expires.is_some_and(|deadline| now >= deadline)
    }
}

#[derive(Default)]
pub struct Db {
    entries: HashMap<Vec<u8>, Entry>,
}

pub type SharedDb = Arc<Mutex<Db>>;

impl Db {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs one command. `now` is a parameter, not `Instant::now()`, so tests
    /// can move time forward without sleeping.
    pub fn execute(&mut self, cmd: Command, now: Instant) -> Frame {
        match cmd {
            Command::Ping(None) => Frame::Simple("PONG".into()),
            Command::Ping(Some(msg)) | Command::Echo(msg) => Frame::Bulk(msg),
            Command::Set(key, value, ttl) => {
                let expires = match ttl {
                    None => None,
                    // `Instant + Duration` panics on overflow; `checked_add` doesn't.
                    Some(ttl) => match now.checked_add(ttl) {
                        Some(deadline) => Some(deadline),
                        None => return error("ERR invalid expire time in 'set' command"),
                    },
                };
                self.entries.insert(key, Entry { value, expires });
                Frame::Simple("OK".into())
            }
            Command::Get(key) => match self.live(&key, now) {
                Some(entry) => Frame::Bulk(entry.value.clone()),
                None => Frame::Null,
            },
            Command::Del(keys) => {
                let mut removed = 0;
                for key in keys {
                    if self.live(&key, now).is_some() {
                        self.entries.remove(&key);
                        removed += 1;
                    }
                }
                Frame::Integer(removed)
            }
            Command::Exists(keys) => {
                let found = keys
                    .iter()
                    .filter(|key| self.live(key, now).is_some())
                    .count();
                Frame::Integer(found as i64)
            }
            Command::Incr(key) => {
                self.expire_if_due(&key, now);
                let entry = self.entries.entry(key).or_insert_with(|| Entry {
                    value: b"0".to_vec(),
                    expires: None,
                });
                // INCR keeps the key's TTL, like Redis.
                match parse_int(&entry.value).and_then(|n| n.checked_add(1)) {
                    Some(n) => {
                        entry.value = n.to_string().into_bytes();
                        Frame::Integer(n)
                    }
                    None => error(NOT_INT),
                }
            }
            Command::Expire(key, secs) => {
                // secs <= 0 sets the deadline to `now`: the key is gone at once.
                let Some(deadline) = now.checked_add(Duration::from_secs(secs.max(0) as u64))
                else {
                    return error("ERR invalid expire time in 'expire' command");
                };
                match self.live(&key, now) {
                    Some(entry) => {
                        entry.expires = Some(deadline);
                        Frame::Integer(1)
                    }
                    None => Frame::Integer(0),
                }
            }
            Command::Ttl(key) => Frame::Integer(match self.live(&key, now) {
                None => -2,
                Some(Entry { expires: None, .. }) => -1,
                Some(Entry {
                    expires: Some(deadline),
                    ..
                }) => {
                    // Round to the nearest second, like Redis.
                    let ms = deadline.duration_since(now).as_millis();
                    i64::try_from((ms + 500) / 1000).unwrap_or(i64::MAX)
                }
            }),
            Command::Keys(pattern) => {
                self.purge_expired(now);
                let mut keys: Vec<&Vec<u8>> = self
                    .entries
                    .keys()
                    .filter(|k| glob_match(&pattern, k))
                    .collect();
                keys.sort(); // HashMap order is random; sorted output is easier to read and test
                Frame::Array(keys.into_iter().map(|k| Frame::Bulk(k.clone())).collect())
            }
            Command::DbSize => {
                self.purge_expired(now);
                Frame::Integer(self.entries.len() as i64)
            }
            Command::FlushAll => {
                self.entries.clear();
                Frame::Simple("OK".into())
            }
        }
    }

    /// Lazy expiry: a key past its deadline is deleted the moment anyone looks.
    fn expire_if_due(&mut self, key: &[u8], now: Instant) {
        if self.entries.get(key).is_some_and(|e| e.expired(now)) {
            self.entries.remove(key);
        }
    }

    /// The entry for `key`, if it exists and hasn't expired.
    fn live(&mut self, key: &[u8], now: Instant) -> Option<&mut Entry> {
        self.expire_if_due(key, now);
        self.entries.get_mut(key)
    }

    fn purge_expired(&mut self, now: Instant) {
        self.entries.retain(|_, entry| !entry.expired(now));
    }
}

fn error(msg: &str) -> Frame {
    Frame::Error(msg.into())
}

/// Glob match with `*` (any run of bytes) and `?` (any one byte).
///
/// Iterative with one backtrack point. The obvious recursive version takes
/// exponential time on patterns like `*a*a*a*a*b`, and the pattern comes
/// from the client.
pub fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    // Where to resume if the current attempt fails: (pattern index just past
    // the last `*`, text index that `*` currently stops at).
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p + 1, t));
                p += 1;
            }
            Some(&c) if c == b'?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            // Mismatch: let the last `*` swallow one more byte and retry.
            _ => match star {
                Some((sp, st)) => {
                    star = Some((sp, st + 1));
                    (p, t) = (sp, st + 1);
                }
                None => return false,
            },
        }
    }
    // Text used up: what's left of the pattern must be all stars.
    pattern[p..].iter().all(|&c| c == b'*')
}

/// Accept loop: one thread per connection. Runs until the listener dies.
/// `idle_timeout` closes connections that stay silent that long.
pub fn serve(listener: TcpListener, db: SharedDb, idle_timeout: Duration) {
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

fn handle(mut stream: TcpStream, db: &SharedDb, idle_timeout: Duration) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    eprintln!("[{peer}] connected");
    stream.set_read_timeout(Some(idle_timeout))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut served = 0;
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) => break, // the client hung up
            Ok(n) => n,
            // A read timeout shows up as WouldBlock on Unix and TimedOut on Windows.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => return Err(e),
        };
        buf.extend_from_slice(&chunk[..n]);

        // Pipelining: answer every complete request in the buffer, in order,
        // then send all the replies with one write.
        let mut replies = Vec::new();
        let mut used = 0;
        let mut fatal = None;
        loop {
            match parse(&buf[used..]) {
                Ok(Some((frame, len))) => {
                    used += len;
                    // A blank line (Enter pressed in nc) gets no reply, like Redis.
                    if frame != Frame::Array(Vec::new()) {
                        respond(frame, db).encode(&mut replies);
                        served += 1;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    fatal = Some(e.0);
                    break;
                }
            }
        }
        // Drop the answered bytes; whatever is left is the start of the next request.
        buf.drain(..used);
        if buf.len() > MAX_BUFFER {
            fatal = Some("request too large");
        }
        if let Some(reason) = fatal {
            Frame::Error(format!("ERR Protocol error: {reason}")).encode(&mut replies);
        }
        stream.write_all(&replies)?;
        if fatal.is_some() {
            break; // like Redis: after garbage, hang up
        }
    }
    eprintln!("[{peer}] disconnected after {served} command(s)");
    Ok(())
}

fn respond(frame: Frame, db: &SharedDb) -> Frame {
    match Command::from_frame(frame) {
        // The lock is held for one command only, never across network I/O.
        Ok(cmd) => db
            .lock()
            .expect("db mutex poisoned")
            .execute(cmd, Instant::now()),
        Err(msg) => Frame::Error(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(frame: &Frame) -> Vec<u8> {
        let mut out = Vec::new();
        frame.encode(&mut out);
        out
    }

    fn bulk(s: &str) -> Frame {
        Frame::Bulk(s.as_bytes().to_vec())
    }

    /// Runs an inline command line against `db` at time `now`.
    fn run(db: &mut Db, now: Instant, line: &str) -> Frame {
        let (frame, _) = parse(format!("{line}\r\n").as_bytes()).unwrap().unwrap();
        match Command::from_frame(frame) {
            Ok(cmd) => db.execute(cmd, now),
            Err(msg) => Frame::Error(msg),
        }
    }

    fn sample_frames() -> Vec<Frame> {
        vec![
            Frame::Simple("OK".into()),
            Frame::Error("ERR boom".into()),
            Frame::Integer(-42),
            bulk(""),
            Frame::Bulk(b"bin\r\nary\0".to_vec()),
            Frame::Null,
            Frame::Array(vec![]),
            Frame::Array(vec![
                bulk("SET"),
                Frame::Integer(1),
                Frame::Array(vec![Frame::Null]),
            ]),
        ]
    }

    #[test]
    fn every_type_round_trips() {
        for frame in sample_frames() {
            let bytes = encoded(&frame);
            assert_eq!(parse(&bytes), Ok(Some((frame, bytes.len()))));
        }
        assert_eq!(encoded(&Frame::Null), b"$-1\r\n");
        // A CR or LF inside an error message must not break the framing.
        let error = Frame::Error("bad\r\nname".into());
        assert_eq!(encoded(&error), b"-bad  name\r\n");
        assert_eq!(parse(b"*-1\r\n"), Ok(Some((Frame::Null, 5))));
    }

    #[test]
    fn every_prefix_is_incomplete() {
        for frame in sample_frames() {
            let bytes = encoded(&frame);
            for cut in 0..bytes.len() {
                assert_eq!(parse(&bytes[..cut]), Ok(None), "{:?}", &bytes[..cut]);
            }
        }
    }

    #[test]
    fn garbage_is_rejected() {
        let deep = "*1\r\n".repeat(MAX_DEPTH + 1);
        let cases: [&[u8]; 8] = [
            b":abc\r\n",
            b"$x\r\n",
            b"$-5\r\n",
            b"$99999999999\r\n", // over MAX_BULK: refused before any bytes arrive
            b"$3\r\nfoobar\r\n", // longer than declared
            b"*-3\r\n",
            b"*1\r\n!what\r\n",
            deep.as_bytes(),
        ];
        for case in cases {
            assert!(parse(case).is_err(), "{:?}", String::from_utf8_lossy(case));
        }
    }

    #[test]
    fn inline_commands_and_pipelined_buffers() {
        let words = |ws: &[&str]| Frame::Array(ws.iter().map(|w| bulk(w)).collect());
        assert_eq!(
            parse(b"SET  a 1\r\n"),
            Ok(Some((words(&["SET", "a", "1"]), 10)))
        );
        assert_eq!(parse(b"PING\n"), Ok(Some((words(&["PING"]), 5)))); // bare LF from nc
        assert_eq!(parse(b"\r\n"), Ok(Some((words(&[]), 2))));
        assert_eq!(parse(b"GET a"), Ok(None)); // no newline yet

        // Two requests in one buffer: parse the first, then the rest.
        let buf = b"*1\r\n$4\r\nPING\r\nECHO hi\r\n";
        let (first, used) = parse(buf).unwrap().unwrap();
        assert_eq!(first, words(&["PING"]));
        assert_eq!(parse(&buf[used..]), Ok(Some((words(&["ECHO", "hi"]), 9))));
    }

    #[test]
    fn strings_and_counters() {
        let (mut db, now) = (Db::new(), Instant::now());
        // Annotating `&str` lets the closure accept temporaries like `&format!(..)`.
        let mut exec = |line: &str| run(&mut db, now, line);
        assert_eq!(exec("PING"), Frame::Simple("PONG".into()));
        assert_eq!(exec("ping hello"), bulk("hello"));
        assert_eq!(exec("ECHO hi"), bulk("hi"));
        assert_eq!(exec("GET a"), Frame::Null);
        assert_eq!(exec("SET a 1"), Frame::Simple("OK".into()));
        assert_eq!(exec("GET a"), bulk("1"));
        assert_eq!(exec("INCR a"), Frame::Integer(2));
        assert_eq!(exec("INCR fresh"), Frame::Integer(1));
        assert_eq!(exec("SET s hello"), Frame::Simple("OK".into()));
        assert_eq!(exec("INCR s"), error(NOT_INT));
        assert_eq!(
            exec(&format!("SET big {}", i64::MAX)),
            Frame::Simple("OK".into())
        );
        assert_eq!(exec("INCR big"), error(NOT_INT));
        assert_eq!(exec("EXISTS a a nope"), Frame::Integer(2));
        assert_eq!(exec("DEL a nope"), Frame::Integer(1));
        assert_eq!(exec("DBSIZE"), Frame::Integer(3));
        assert_eq!(exec("FLUSHALL"), Frame::Simple("OK".into()));
        assert_eq!(exec("DBSIZE"), Frame::Integer(0));
    }

    #[test]
    fn keys_with_globs() {
        let cases = [
            ("*", "", true),
            ("user:*", "user:22", true),
            ("user:?", "user:22", false),
            ("u?er:*", "user:1", true),
            ("a*b*c", "aXXbYYc", true),
            ("a?", "a", false),
        ];
        for (pattern, text, want) in cases {
            assert_eq!(
                glob_match(pattern.as_bytes(), text.as_bytes()),
                want,
                "{pattern} {text}"
            );
        }
        // Instant, even though naive backtracking would take forever here.
        assert!(!glob_match(b"*a*a*a*a*a*a*a*a*b", &[b'a'; 60]));

        let (mut db, now) = (Db::new(), Instant::now());
        for key in ["user:1", "user:22", "admin", "use"] {
            run(&mut db, now, &format!("SET {key} x"));
        }
        let want = Frame::Array(vec![bulk("use"), bulk("user:1"), bulk("user:22")]);
        assert_eq!(run(&mut db, now, "KEYS u*"), want);
        assert_eq!(run(&mut db, now, "KEYS nothing*"), Frame::Array(vec![]));
    }

    #[test]
    fn expiry_with_a_fake_clock() {
        let (mut db, t0) = (Db::new(), Instant::now());
        let ms = Duration::from_millis;
        assert_eq!(
            run(&mut db, t0, "SET a 1 PX 100"),
            Frame::Simple("OK".into())
        );
        assert_eq!(run(&mut db, t0 + ms(99), "GET a"), bulk("1"));
        assert_eq!(run(&mut db, t0 + ms(100), "GET a"), Frame::Null);
        assert_eq!(run(&mut db, t0 + ms(100), "TTL a"), Frame::Integer(-2));

        run(&mut db, t0, "SET b 1 EX 10");
        assert_eq!(run(&mut db, t0 + ms(2400), "TTL b"), Frame::Integer(8)); // 7.6 s rounds to 8
        run(&mut db, t0, "INCR b"); // INCR keeps the TTL
        assert_eq!(run(&mut db, t0 + ms(10_000), "EXISTS b"), Frame::Integer(0));

        run(&mut db, t0, "SET c 1");
        assert_eq!(run(&mut db, t0, "TTL c"), Frame::Integer(-1));
        assert_eq!(run(&mut db, t0, "EXPIRE c 5"), Frame::Integer(1));
        assert_eq!(run(&mut db, t0, "TTL c"), Frame::Integer(5));
        assert_eq!(run(&mut db, t0, "EXPIRE nope 5"), Frame::Integer(0));
        assert_eq!(run(&mut db, t0, "EXPIRE c 0"), Frame::Integer(1)); // deletes at once
        assert_eq!(run(&mut db, t0, "DBSIZE"), Frame::Integer(0));
    }

    #[test]
    fn bad_commands() {
        let (mut db, now) = (Db::new(), Instant::now());
        let mut exec = |line: &str| run(&mut db, now, line);
        assert_eq!(
            exec("GET"),
            error("ERR wrong number of arguments for 'get' command")
        );
        assert_eq!(
            exec("DEL"),
            error("ERR wrong number of arguments for 'del' command")
        );
        assert_eq!(exec("FLY away"), error("ERR unknown command 'FLY'"));
        assert_eq!(exec("SET a 1 EX"), error(SYNTAX));
        assert_eq!(exec("SET a 1 XX 5"), error(SYNTAX));
        assert_eq!(
            exec("SET a 1 EX 0"),
            error("ERR invalid expire time in 'set' command")
        );
        assert_eq!(exec("SET a 1 PX soon"), error(NOT_INT));
        assert_eq!(exec("EXPIRE a soon"), error(NOT_INT));
        assert_eq!(
            exec(&format!("EXPIRE a {}", i64::MAX)),
            error("ERR invalid expire time in 'expire' command")
        );
        assert!(Command::from_frame(Frame::Integer(1)).is_err());
        assert!(Command::from_frame(Frame::Array(vec![Frame::Integer(1)])).is_err());
    }
}
