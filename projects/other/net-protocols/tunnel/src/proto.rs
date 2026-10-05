//! The control protocol: one message per `\n`-terminated text line.
//! Pure parsing and formatting, plus two ways to read lines off a socket.

use std::fmt;
use std::io;
use std::str::FromStr;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// Control lines are short. Anything longer is garbage or an attack.
pub const MAX_LINE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Hello(String),
    Ok,
    Err(String),
    Connect(u64),
    Data(u64),
    Ping,
    Pong,
}

/// Formatting and parsing are each other's inverse: `msg.to_string().parse()`
/// gives back `msg`. The unit tests check exactly that.
impl fmt::Display for Msg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Msg::Hello(token) => write!(f, "HELLO {token}"),
            Msg::Ok => write!(f, "OK"),
            Msg::Err(reason) => write!(f, "ERR {reason}"),
            Msg::Connect(id) => write!(f, "CONNECT {id}"),
            Msg::Data(id) => write!(f, "DATA {id}"),
            Msg::Ping => write!(f, "PING"),
            Msg::Pong => write!(f, "PONG"),
        }
    }
}

impl FromStr for Msg {
    type Err = String;

    fn from_str(line: &str) -> Result<Msg, String> {
        let (word, arg) = match line.split_once(' ') {
            Some((word, arg)) => (word, Some(arg)),
            None => (line, None),
        };
        let id = |arg: &str| {
            arg.parse::<u64>()
                .map_err(|_| format!("bad id in {line:?}"))
        };
        match (word, arg) {
            ("HELLO", Some(token)) if !token.is_empty() => Ok(Msg::Hello(token.to_string())),
            ("OK", None) => Ok(Msg::Ok),
            ("ERR", Some(reason)) => Ok(Msg::Err(reason.to_string())),
            ("CONNECT", Some(arg)) => id(arg).map(Msg::Connect),
            ("DATA", Some(arg)) => id(arg).map(Msg::Data),
            ("PING", None) => Ok(Msg::Ping),
            ("PONG", None) => Ok(Msg::Pong),
            _ => Err(format!("unknown message {line:?}")),
        }
    }
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Bytes of one line (without the `\n`) to a message. A `\r` is tolerated,
/// so you can type the protocol by hand with `nc -c`.
fn parse_line(line: &[u8]) -> io::Result<Msg> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let text = std::str::from_utf8(line).map_err(|_| invalid("control line is not UTF-8"))?;
    text.parse().map_err(invalid)
}

pub async fn send<W: AsyncWrite + Unpin>(writer: &mut W, msg: &Msg) -> io::Result<()> {
    writer.write_all(format!("{msg}\n").as_bytes()).await
}

/// Reads the first line of a new connection **one byte at a time**.
///
/// That is slow, but it never reads past the `\n`. On a data connection the
/// bytes right after `DATA <id>\n` belong to the tunneled stream. A
/// `BufReader` would happily pull them into its buffer, and they'd be lost
/// when we hand the bare `TcpStream` over to be spliced.
pub async fn read_first_line<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Msg> {
    let mut line = Vec::new();
    loop {
        // `read_u8` turns EOF into an `UnexpectedEof` error, which is right:
        // a connection that closes before finishing its first line is broken.
        let byte = reader.read_u8().await?;
        if byte == b'\n' {
            return parse_line(&line);
        }
        if line.len() == MAX_LINE {
            return Err(invalid("first line too long"));
        }
        line.push(byte);
    }
}

/// Buffered message reader for the rest of a control connection.
///
/// Cancel-safe, so it can sit in a `select!` branch: `read_until` appends
/// whatever it has read to `self.buf` as it goes. If the future is dropped
/// halfway through a line, those bytes stay in `buf`, and the next call
/// carries on with the same line. (tokio's `read_line` doesn't promise that.)
pub struct MsgReader<R> {
    inner: BufReader<R>,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> MsgReader<R> {
    pub fn new(reader: R) -> Self {
        MsgReader {
            inner: BufReader::new(reader),
            buf: Vec::new(),
        }
    }

    /// The next message, or `None` at EOF. Over-long lines and unknown
    /// messages are errors; the caller closes the connection.
    pub async fn next(&mut self) -> io::Result<Option<Msg>> {
        // `take` caps how far this call may read, so `buf` never grows past
        // MAX_LINE + 1 bytes, however long the peer's line is.
        let room = (MAX_LINE + 1).saturating_sub(self.buf.len()) as u64;
        (&mut self.inner)
            .take(room)
            .read_until(b'\n', &mut self.buf)
            .await?;

        if self.buf.last() == Some(&b'\n') {
            let line = std::mem::take(&mut self.buf);
            return parse_line(&line[..line.len() - 1]).map(Some);
        }
        if self.buf.len() > MAX_LINE {
            self.buf.clear();
            return Err(invalid("control line too long"));
        }
        // `read_until` stops only at `\n`, at the `take` limit, or at EOF.
        // We've ruled out the first two, so this is EOF.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn every_message_round_trips() {
        let all = [
            Msg::Hello("s3cret token".into()),
            Msg::Ok,
            Msg::Err("bad token".into()),
            Msg::Connect(0),
            Msg::Data(u64::MAX),
            Msg::Ping,
            Msg::Pong,
        ];
        for msg in all {
            assert_eq!(msg.to_string().parse::<Msg>(), Ok(msg.clone()), "{msg}");
        }
    }

    #[test]
    fn rejects_junk() {
        for bad in [
            "",
            "HELLO",
            "HELLO ",
            "CONNECT",
            "CONNECT x",
            "DATA -1",
            "PING now",
            "OK go",
            "hello dev",
            "FOO 1",
        ] {
            assert!(bad.parse::<Msg>().is_err(), "{bad:?} should not parse");
        }
    }

    #[tokio::test]
    async fn first_line_reader_leaves_the_rest_alone() {
        let mut input: &[u8] = b"DATA 7\nGET / HTTP/1.1\r\n";
        assert_eq!(read_first_line(&mut input).await.unwrap(), Msg::Data(7));
        assert_eq!(input, b"GET / HTTP/1.1\r\n");

        let long = vec![b'x'; 10_000];
        assert!(read_first_line(&mut &long[..]).await.is_err());
        assert!(read_first_line(&mut &b"PING"[..]).await.is_err()); // EOF mid-line
    }

    #[tokio::test]
    async fn msg_reader_reads_lines_and_rejects_bad_ones() {
        let mut ok = MsgReader::new(&b"OK\r\nCONNECT 3\nPONG\n"[..]);
        assert_eq!(ok.next().await.unwrap(), Some(Msg::Ok));
        assert_eq!(ok.next().await.unwrap(), Some(Msg::Connect(3)));
        assert_eq!(ok.next().await.unwrap(), Some(Msg::Pong));
        assert_eq!(ok.next().await.unwrap(), None);

        let long = [vec![b'A'; 1000], b"\n".to_vec()].concat();
        assert!(MsgReader::new(&long[..]).next().await.is_err());
        assert!(MsgReader::new(&b"WHAT\n"[..]).next().await.is_err());
        assert!(MsgReader::new(&b"\xff\n"[..]).next().await.is_err());
    }

    #[tokio::test]
    async fn msg_reader_survives_cancellation() {
        let (mut agent, relay) = tokio::io::duplex(64);
        let mut msgs = MsgReader::new(relay);
        agent.write_all(b"CONN").await.unwrap();
        // Lose a race, as a select! branch would: the read is dropped mid-line.
        let lost = tokio::time::timeout(Duration::from_millis(20), msgs.next()).await;
        assert!(lost.is_err());
        agent.write_all(b"ECT 9\n").await.unwrap();
        assert_eq!(msgs.next().await.unwrap(), Some(Msg::Connect(9)));
    }
}
