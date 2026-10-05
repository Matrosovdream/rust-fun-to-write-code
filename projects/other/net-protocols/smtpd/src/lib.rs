//! smtpd — an SMTP mail sink: accepts mail for anyone and saves it to disk.
//!
//! # Protocol (a subset of RFC 5321)
//!
//! SMTP is a lock-step, line-based conversation. The client sends one command
//! line, and the server answers with a three-digit code plus some text. The
//! first digit tells the client how it went: `2xx` done, `3xx` go on, `4xx`
//! try again later, `5xx` failed. Lines end in CRLF (`\r\n`). This server
//! also accepts a bare `\n` so plain `nc` works, but it always sends CRLF.
//!
//! ```text
//! S: 220 smtpd ESMTP ready
//! C: EHLO laptop
//! S: 250-smtpd                         "250-" means more lines follow,
//! S: 250 SIZE 1048576                  "250 " marks the last one
//! C: MAIL FROM:<alice@example.com>
//! S: 250 OK
//! C: RCPT TO:<bob@example.com>         (repeat for more recipients)
//! S: 250 OK
//! C: DATA
//! S: 354 end data with <CR><LF>.<CR><LF>
//! C: Subject: hi
//! C:
//! C: ..this body line really starts with one dot
//! C: .
//! S: 250 OK: queued as 1759670000000-0
//! C: QUIT
//! S: 221 bye
//! ```
//!
//! A line holding just `.` ends the message, so the client "dot-stuffs" body
//! lines that start with a dot (`.x` is sent as `..x`). The server removes
//! that extra dot again.
//!
//! Other commands: `HELO` (the pre-ESMTP greeting), `RSET` (abandon the
//! current message), `NOOP`. Error codes: `500` unknown command or line too
//! long, `501` bad arguments, `503` command out of order, `452` too many
//! recipients, `552` message too large, `421` idle timeout.
//!
//! # Design
//!
//! [`Session`] is the protocol as a state machine. [`Session::step`] takes
//! the current state *by value* along with one input line, and returns the
//! next state plus an [`Outcome`]. It does no I/O, so every transition is
//! unit-testable. [`serve`] does the I/O: it reads lines, feeds `step`,
//! writes replies, and saves finished messages as `.eml` files.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Longest accepted line, counting every byte before the `\n`.
/// (RFC 5321 says 1000 including the CRLF, so we're a little lenient.)
pub const MAX_LINE: usize = 1000;
/// Largest message we store. We advertise it in the EHLO reply as `SIZE`.
pub const MAX_MESSAGE: usize = 1024 * 1024;
/// RFC 5321 says a server must accept at least 100 recipients per message.
pub const MAX_RCPTS: usize = 100;

/// Where the conversation is. Each variant carries exactly the data that is
/// valid in that state: there's no "sender: Option<String>" that might be
/// unset when we need it, and `DATA` before `RCPT` simply has no variant.
#[derive(Debug, Clone, PartialEq)]
pub enum Session {
    /// Waiting for HELO/EHLO.
    Connected,
    /// Greeted, no message in progress.
    Ready,
    Mail {
        from: String,
    },
    Rcpt {
        from: String,
        to: Vec<String>,
    },
    /// Receiving the body. `too_big` flips once the body passes
    /// [`MAX_MESSAGE`]. After that we drop the data but keep reading up to
    /// the final `.`, so the client gets a clean `552`.
    Data {
        from: String,
        to: Vec<String>,
        body: Vec<u8>,
        too_big: bool,
    },
}

/// A complete message, ready to be saved.
#[derive(Debug, PartialEq)]
pub struct Message {
    pub from: String,
    pub to: Vec<String>,
    /// Lines joined with CRLF, already dot-unstuffed.
    pub body: Vec<u8>,
}

/// What the I/O layer should do after a step.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Reply(Reply),
    /// A body line was absorbed: nothing to say yet.
    Silent,
    /// The message is complete: save it, then reply `250` with its id.
    Deliver(Message),
    /// Send the reply, then hang up.
    Close(Reply),
}

#[derive(Debug, PartialEq)]
pub struct Reply {
    pub code: u16,
    /// One or more lines, separated by `\n`.
    pub text: String,
}

impl Reply {
    pub fn new(code: u16, text: impl Into<String>) -> Reply {
        Reply {
            code,
            text: text.into(),
        }
    }

    /// Wire format: every line repeats the code. A `-` after the code means
    /// "more lines follow" and a space marks the last line.
    pub fn encode(&self) -> String {
        let lines: Vec<&str> = self.text.split('\n').collect();
        let last = lines.len() - 1; // split always yields at least one piece
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            let sep = if i == last { ' ' } else { '-' };
            out.push_str(&format!("{}{sep}{line}\r\n", self.code));
        }
        out
    }
}

#[derive(Debug, PartialEq)]
enum Command {
    Helo,
    Ehlo,
    Mail(String),
    Rcpt(String),
    Data,
    Rset,
    Noop,
    Quit,
}

/// Parses a command line. `Err` holds the `500`/`501` reply to send back.
fn parse_command(line: &str) -> Result<Command, Reply> {
    let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
    // Verbs are case-insensitive: `mail from:<a@b>` is fine.
    match verb.to_ascii_uppercase().as_str() {
        "HELO" => Ok(Command::Helo),
        "EHLO" => Ok(Command::Ehlo),
        "MAIL" => match parse_path(arg, "FROM:") {
            Some(from) => Ok(Command::Mail(from)),
            None => Err(Reply::new(501, "syntax: MAIL FROM:<address>")),
        },
        "RCPT" => match parse_path(arg, "TO:") {
            Some(to) if !to.is_empty() => Ok(Command::Rcpt(to)),
            _ => Err(Reply::new(501, "syntax: RCPT TO:<address>")),
        },
        "DATA" => Ok(Command::Data),
        "RSET" => Ok(Command::Rset),
        "NOOP" => Ok(Command::Noop),
        "QUIT" => Ok(Command::Quit),
        _ => Err(Reply::new(500, "unknown command")),
    }
}

/// `FROM:<alice@example.com> SIZE=123` gives `alice@example.com`. Anything
/// after the `>` is ignored. `<>` (the null sender that bounces use) gives "".
fn parse_path(arg: &str, prefix: &str) -> Option<String> {
    // `get` returns None instead of panicking when the cut would land inside
    // a multi-byte character.
    if !arg.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = arg[prefix.len()..].trim_start().strip_prefix('<')?;
    let addr = &rest[..rest.find('>')?];
    // A stray \r or other control character would corrupt the saved headers.
    if addr.chars().any(char::is_control) {
        return None;
    }
    Some(addr.to_string())
}

fn reply(code: u16, text: impl Into<String>) -> Outcome {
    Outcome::Reply(Reply::new(code, text))
}

fn ok() -> Outcome {
    reply(250, "OK")
}

impl Session {
    /// One step of the state machine. `self` is consumed, so the old state
    /// can't be used by mistake: the caller must take the returned one.
    pub fn step(self, line: &[u8]) -> (Session, Outcome) {
        match self {
            Session::Data {
                from,
                to,
                body,
                too_big,
            } => data_line(from, to, body, too_big, line),
            state => state.command(line),
        }
    }

    fn command(self, line: &[u8]) -> (Session, Outcome) {
        if line.len() > MAX_LINE {
            return (self, reply(500, "line too long"));
        }
        let cmd = match parse_command(String::from_utf8_lossy(line).trim()) {
            Ok(cmd) => cmd,
            Err(error) => return (self, Outcome::Reply(error)),
        };
        // Match on (state, command) pairs. Arms are tried top to bottom, so the
        // 503 arms at the end only see commands sent in the wrong order.
        match (self, cmd) {
            (state, Command::Quit) => (state, Outcome::Close(Reply::new(221, "bye"))),
            (state, Command::Noop) => (state, ok()),
            (Session::Connected, Command::Rset) => (Session::Connected, ok()),
            (_, Command::Rset) => (Session::Ready, ok()),
            // A (re)greeting also abandons any message in progress.
            (_, Command::Helo) => (Session::Ready, reply(250, "smtpd")),
            (_, Command::Ehlo) => (
                Session::Ready,
                reply(250, format!("smtpd\nSIZE {MAX_MESSAGE}")),
            ),
            (Session::Connected, _) => (Session::Connected, reply(503, "send HELO or EHLO first")),
            (Session::Ready, Command::Mail(from)) => (Session::Mail { from }, ok()),
            (Session::Mail { from }, Command::Rcpt(addr)) => (
                Session::Rcpt {
                    from,
                    to: vec![addr],
                },
                ok(),
            ),
            (Session::Rcpt { from, mut to }, Command::Rcpt(addr)) => {
                let outcome = if to.len() < MAX_RCPTS {
                    to.push(addr);
                    ok()
                } else {
                    reply(452, "too many recipients")
                };
                (Session::Rcpt { from, to }, outcome)
            }
            (Session::Rcpt { from, to }, Command::Data) => (
                Session::Data {
                    from,
                    to,
                    body: Vec::new(),
                    too_big: false,
                },
                reply(354, "end data with <CR><LF>.<CR><LF>"),
            ),
            (state, Command::Mail(_)) => (state, reply(503, "nested MAIL command")),
            (state, Command::Rcpt(_)) => (state, reply(503, "need MAIL before RCPT")),
            (state, Command::Data) => (state, reply(503, "need RCPT before DATA")),
        }
    }
}

/// One line inside DATA: the end marker or one more body line.
fn data_line(
    from: String,
    to: Vec<String>,
    mut body: Vec<u8>,
    mut too_big: bool,
    line: &[u8],
) -> (Session, Outcome) {
    if line == b"." {
        let outcome = if too_big {
            reply(552, "message too large")
        } else {
            Outcome::Deliver(Message { from, to, body })
        };
        return (Session::Ready, outcome);
    }
    // The reader cut this line short, so we couldn't store it faithfully.
    let cut = line.len() > MAX_LINE;
    // Undo dot-stuffing: the client doubled any leading dot.
    let line = line.strip_prefix(b".").unwrap_or(line);
    if too_big || cut || body.len() + line.len() + 2 > MAX_MESSAGE {
        too_big = true;
        body = Vec::new(); // free what we had
    } else {
        body.extend_from_slice(line);
        body.extend_from_slice(b"\r\n"); // stored as CRLF, whatever the client sent
    }
    (
        Session::Data {
            from,
            to,
            body,
            too_big,
        },
        Outcome::Silent,
    )
}

/// The saved file: envelope headers (who the mail was *really* from and to,
/// which can differ from the `From:`/`To:` headers inside), then the message.
pub fn to_eml(msg: &Message) -> Vec<u8> {
    let to: Vec<String> = msg.to.iter().map(|t| format!("<{t}>")).collect();
    let head = format!(
        "X-Envelope-From: <{}>\r\nX-Envelope-To: {}\r\n",
        msg.from,
        to.join(", ")
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(&msg.body);
    out
}

/// Saves `msg` as `dir/<id>.eml`, creating `dir` if needed. Returns the id.
pub fn save(dir: &Path, msg: &Message) -> io::Result<String> {
    // A millisecond timestamp plus a process-wide counter: unique even when
    // two connections finish a message in the same millisecond.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let id = format!("{millis}-{}", COUNTER.fetch_add(1, Ordering::Relaxed));
    fs::create_dir_all(dir)?;
    fs::write(dir.join(format!("{id}.eml")), to_eml(msg))?;
    Ok(id)
}

/// Reads one line without its `\n` / `\r\n`. Returns `Ok(None)` at EOF.
///
/// The peer decides how long a line is, so we never buffer more than
/// `max + 1` bytes of it. If the line is longer, the rest is read and thrown
/// away, and the caller can spot it with `line.len() > max`.
pub fn read_line_limited(r: &mut impl BufRead, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    // `take` caps how many bytes `read_until` may consume.
    r.by_ref()
        .take(max as u64 + 1)
        .read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    } else if line.len() > max {
        // Discard the rest of the line, one buffer at a time.
        loop {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            if let Some(i) = buf.iter().position(|&b| b == b'\n') {
                r.consume(i + 1);
                break;
            }
            let n = buf.len();
            r.consume(n);
        }
    }
    Ok(Some(line))
}

pub struct Config {
    /// Where `.eml` files go. Created on the first delivery.
    pub maildir: PathBuf,
    /// How long a client may stay silent before we send `421` and hang up.
    pub timeout: Duration,
}

/// Accept loop: one thread per connection. Runs until the listener dies.
pub fn serve(listener: TcpListener, config: Config) {
    let config = Arc::new(config);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let config = Arc::clone(&config);
                thread::spawn(move || {
                    if let Err(e) = handle(stream, &config) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

fn send(w: &mut impl Write, reply: &Reply) -> io::Result<()> {
    w.write_all(reply.encode().as_bytes())
}

fn handle(stream: TcpStream, config: &Config) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    eprintln!("[{peer}] connected");
    // Without a timeout a silent client would hold this thread forever.
    stream.set_read_timeout(Some(config.timeout))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    send(&mut writer, &Reply::new(220, "smtpd ESMTP ready"))?;
    let mut session = Session::Connected;
    loop {
        let line = match read_line_limited(&mut reader, MAX_LINE) {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()), // the client hung up
            // A read timeout shows up as WouldBlock on Unix and TimedOut on Windows.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return send(&mut writer, &Reply::new(421, "idle timeout, closing"));
            }
            Err(e) => return Err(e),
        };
        let (next, outcome) = session.step(&line);
        session = next;
        match outcome {
            Outcome::Silent => {}
            Outcome::Reply(reply) => send(&mut writer, &reply)?,
            Outcome::Deliver(msg) => {
                let reply = match save(&config.maildir, &msg) {
                    Ok(id) => {
                        eprintln!(
                            "[{peer}] queued {id} from <{}> to {} recipient(s)",
                            msg.from,
                            msg.to.len()
                        );
                        Reply::new(250, format!("OK: queued as {id}"))
                    }
                    Err(e) => {
                        eprintln!("[{peer}] could not save message: {e}");
                        Reply::new(451, "local error, try again later")
                    }
                };
                send(&mut writer, &reply)?;
            }
            Outcome::Close(reply) => return send(&mut writer, &reply),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn mail() -> Session {
        Session::Mail { from: "a@x".into() }
    }

    fn rcpt() -> Session {
        Session::Rcpt {
            from: "a@x".into(),
            to: vec!["b@y".into()],
        }
    }

    fn code(outcome: &Outcome) -> u16 {
        match outcome {
            Outcome::Reply(r) | Outcome::Close(r) => r.code,
            Outcome::Deliver(_) => 250,
            Outcome::Silent => 0,
        }
    }

    fn kind(session: &Session) -> &'static str {
        match session {
            Session::Connected => "Conn",
            Session::Ready => "Ready",
            Session::Mail { .. } => "Mail",
            Session::Rcpt { .. } => "Rcpt",
            Session::Data { .. } => "Data",
        }
    }

    /// Feeds lines from a fresh connection; returns the final state and every outcome.
    fn run(lines: &[&str]) -> (Session, Vec<Outcome>) {
        let mut session = Session::Connected;
        let mut outcomes = Vec::new();
        for line in lines {
            let (next, outcome) = session.step(line.as_bytes());
            session = next;
            outcomes.push(outcome);
        }
        (session, outcomes)
    }

    const UP_TO_DATA: [&str; 4] = ["HELO me", "MAIL FROM:<a@x>", "RCPT TO:<b@y>", "DATA"];

    /// Every command in every command state. Each cell is "<reply code> <next state>".
    #[test]
    fn transition_table() {
        let commands = [
            "HELO h",
            "EHLO h",
            "MAIL FROM:<c@x>",
            "RCPT TO:<d@y>",
            "DATA",
            "RSET",
            "NOOP",
            "QUIT",
            "VRFY x",
        ];
        #[rustfmt::skip]
        let table = [
            //                      HELO         EHLO         MAIL         RCPT         DATA         RSET         NOOP         QUIT         VRFY
            (Session::Connected,  ["250 Ready", "250 Ready", "503 Conn",  "503 Conn",  "503 Conn",  "250 Conn",  "250 Conn",  "221 Conn",  "500 Conn"]),
            (Session::Ready,      ["250 Ready", "250 Ready", "250 Mail",  "503 Ready", "503 Ready", "250 Ready", "250 Ready", "221 Ready", "500 Ready"]),
            (mail(),              ["250 Ready", "250 Ready", "503 Mail",  "250 Rcpt",  "503 Mail",  "250 Ready", "250 Mail",  "221 Mail",  "500 Mail"]),
            (rcpt(),              ["250 Ready", "250 Ready", "503 Rcpt",  "250 Rcpt",  "354 Data",  "250 Ready", "250 Rcpt",  "221 Rcpt",  "500 Rcpt"]),
        ];
        for (start, row) in table {
            for (cmd, want) in commands.iter().zip(row) {
                // `step` consumes the state, so each cell gets its own clone.
                let (next, outcome) = start.clone().step(cmd.as_bytes());
                let got = format!("{} {}", code(&outcome), kind(&next));
                assert_eq!(got, want, "{} + {cmd}", kind(&start));
            }
        }
    }

    #[test]
    fn full_transaction_delivers_with_unstuffed_crlf_body() {
        let (state, outcomes) = run(&[
            "ehlo me",
            "mail from: <alice@example.com> SIZE=99",
            "RCPT TO:<bob@example.com>",
            "RCPT TO:<carol@example.com>",
            "DATA",
            "Subject: hi",
            "",
            "..one dot",
            "...",
            ".",
        ]);
        assert_eq!(state, Session::Ready);
        let Some(Outcome::Deliver(msg)) = outcomes.last() else {
            panic!("{outcomes:?}")
        };
        assert_eq!(msg.from, "alice@example.com");
        assert_eq!(msg.to, ["bob@example.com", "carol@example.com"]);
        assert_eq!(msg.body, b"Subject: hi\r\n\r\n.one dot\r\n..\r\n");
        assert_eq!(code(&outcomes[1]), 250);
        assert!(outcomes[5..9].iter().all(|o| *o == Outcome::Silent));
    }

    #[test]
    fn oversized_message_gets_552_after_the_dot() {
        let line = "a".repeat(998);
        let mut lines = UP_TO_DATA.to_vec();
        lines.extend(std::iter::repeat_n(line.as_str(), MAX_MESSAGE / 1000 + 5));
        lines.push(".");
        let (state, outcomes) = run(&lines);
        assert_eq!(state, Session::Ready);
        assert_eq!(code(outcomes.last().unwrap()), 552);
        // Nothing was said in the middle of the data.
        assert!(
            outcomes[4..outcomes.len() - 1]
                .iter()
                .all(|o| *o == Outcome::Silent)
        );

        // One over-long line is enough too.
        let long = "b".repeat(MAX_LINE + 1);
        let (_, outcomes) = run(&[&UP_TO_DATA[..], &[long.as_str(), "."][..]].concat());
        assert_eq!(code(outcomes.last().unwrap()), 552);
    }

    #[test]
    fn bad_arguments_and_limits() {
        let long = "x".repeat(MAX_LINE + 1);
        let (_, outcomes) = run(&[
            "HELO me",
            "MAIL FROM:alice",
            "MAIL FROM:<a@x>",
            "RCPT TO:<>",
            long.as_str(),
        ]);
        let codes: Vec<u16> = outcomes.iter().map(code).collect();
        assert_eq!(codes, [250, 501, 250, 501, 500]);

        let mut lines = vec!["HELO me", "MAIL FROM:<>"]; // null sender is allowed
        lines.extend(std::iter::repeat_n("RCPT TO:<b@y>", MAX_RCPTS + 1));
        let (state, outcomes) = run(&lines);
        assert_eq!(code(outcomes.last().unwrap()), 452);
        assert!(matches!(state, Session::Rcpt { to, .. } if to.len() == MAX_RCPTS));
    }

    #[test]
    fn parse_path_cases() {
        let p = |s| parse_path(s, "FROM:");
        assert_eq!(p("FROM:<a@b>").as_deref(), Some("a@b"));
        assert_eq!(p("from: <a@b> SIZE=10").as_deref(), Some("a@b"));
        assert_eq!(p("FROM:<>").as_deref(), Some(""));
        for bad in ["FROM:a@b", "FROM:<a@b", "TO:<a@b>", "FROM:<a\rb>", "", "é"] {
            assert_eq!(p(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn reply_encoding_and_eml() {
        assert_eq!(Reply::new(250, "OK").encode(), "250 OK\r\n");
        assert_eq!(
            Reply::new(250, "smtpd\nSIZE 10").encode(),
            "250-smtpd\r\n250 SIZE 10\r\n"
        );
        let msg = Message {
            from: "a@x".into(),
            to: vec!["b@y".into(), "c@z".into()],
            body: b"hi\r\n".to_vec(),
        };
        assert_eq!(
            to_eml(&msg),
            b"X-Envelope-From: <a@x>\r\nX-Envelope-To: <b@y>, <c@z>\r\nhi\r\n"
        );
    }

    #[test]
    fn read_line_limited_cuts_long_lines() {
        let input = format!("HELO a\r\nNOOP\n{}\nQUIT", "x".repeat(MAX_LINE * 3));
        let mut r = BufReader::with_capacity(16, Cursor::new(input));
        let mut next = || read_line_limited(&mut r, MAX_LINE).unwrap();
        assert_eq!(next().unwrap(), b"HELO a");
        assert_eq!(next().unwrap(), b"NOOP");
        assert_eq!(next().unwrap().len(), MAX_LINE + 1);
        assert_eq!(next().unwrap(), b"QUIT");
        assert_eq!(next(), None);
    }
}
