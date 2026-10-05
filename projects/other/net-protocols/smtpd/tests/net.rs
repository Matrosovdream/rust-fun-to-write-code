//! Integration tests over real localhost sockets.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use smtpd::{Config, serve};
use tempfile::TempDir;

/// Binds port 0 (the OS picks a free one) and serves into a fresh temp dir.
fn start_server(timeout: Duration) -> (SocketAddr, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Config {
        maildir: dir.path().join("maildir"),
        timeout,
    };
    thread::spawn(move || serve(listener, config));
    (addr, dir)
}

struct Conn {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Conn {
    fn open(addr: SocketAddr) -> Conn {
        let stream = TcpStream::connect(addr).unwrap();
        // A bug should fail the test, not hang it.
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        Conn {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
        }
    }

    /// One reply line. The server must always end it with CRLF.
    fn line(&mut self) -> String {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        assert!(line.ends_with("\r\n"), "not CRLF-terminated: {line:?}");
        line.trim_end().to_string()
    }

    /// Sends a command with a bare LF, the way plain `nc` would.
    fn cmd(&mut self, line: &str) -> String {
        self.writer
            .write_all(format!("{line}\n").as_bytes())
            .unwrap();
        self.line()
    }

    fn assert_closed(&mut self) {
        let mut rest = Vec::new();
        assert_eq!(self.reader.read_to_end(&mut rest).unwrap(), 0);
    }
}

#[test]
fn full_session_saves_an_eml_file() {
    let (addr, dir) = start_server(Duration::from_secs(5));
    let mut c = Conn::open(addr);
    assert_eq!(c.line(), "220 smtpd ESMTP ready");
    assert_eq!(
        c.cmd("MAIL FROM:<alice@example.com>"),
        "503 send HELO or EHLO first"
    );
    assert_eq!(c.cmd("EHLO test"), "250-smtpd");
    assert_eq!(c.line(), "250 SIZE 1048576");
    assert_eq!(c.cmd("MAIL FROM:<alice@example.com>"), "250 OK");
    assert_eq!(c.cmd("DATA"), "503 need RCPT before DATA");
    assert_eq!(c.cmd("RCPT TO:<bob@example.com>"), "250 OK");
    assert_eq!(c.cmd("RCPT TO:<carol@example.com>"), "250 OK");
    assert!(c.cmd("DATA").starts_with("354 "));

    // CRLF and bare LF mixed, plus a dot-stuffed line.
    c.writer
        .write_all(b"Subject: test\r\n\r\nhello\n..leading dot\r\n.\r\n")
        .unwrap();
    let queued = c.line();
    let id = queued
        .strip_prefix("250 OK: queued as ")
        .expect("queued reply");
    assert_eq!(c.cmd("QUIT"), "221 bye");
    c.assert_closed();

    // The file is written before the 250 is sent, so it must exist by now.
    let saved = fs::read_to_string(dir.path().join("maildir").join(format!("{id}.eml"))).unwrap();
    assert_eq!(
        saved,
        "X-Envelope-From: <alice@example.com>\r\n\
         X-Envelope-To: <bob@example.com>, <carol@example.com>\r\n\
         Subject: test\r\n\r\nhello\r\n.leading dot\r\n"
    );
}

#[test]
fn rset_abandons_the_message_and_garbage_is_rejected() {
    let (addr, dir) = start_server(Duration::from_secs(5));
    let mut c = Conn::open(addr);
    c.line();
    assert_eq!(c.cmd("HELO test"), "250 smtpd");
    assert_eq!(c.cmd("MAIL FROM:<a@x>"), "250 OK");
    assert_eq!(c.cmd("RSET"), "250 OK");
    assert_eq!(c.cmd("RCPT TO:<b@y>"), "503 need MAIL before RCPT");
    assert_eq!(c.cmd("\u{1}\u{2} hello?"), "500 unknown command");
    assert_eq!(c.cmd(&"A".repeat(100_000)), "500 line too long");
    assert_eq!(c.cmd("NOOP"), "250 OK");
    assert!(!dir.path().join("maildir").exists()); // nothing was delivered
}

#[test]
fn silent_client_gets_421_and_is_dropped() {
    let (addr, _dir) = start_server(Duration::from_millis(100));
    let mut c = Conn::open(addr);
    c.line(); // greeting
    assert_eq!(c.line(), "421 idle timeout, closing");
    c.assert_closed();
}
