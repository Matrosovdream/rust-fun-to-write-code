//! websock-client: a terminal chat client. Lines from stdin go out as text
//! frames, and incoming messages are printed to stdout.

use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::thread;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use websock::frame::{Assembler, CLOSE_NO_STATUS, CLOSE_NORMAL, Frame, Message, Opcode, WsError};
use websock::handshake::{check_response, client_request, read_head};

const USAGE: &str = "usage: websock-client [ADDR]    (default 127.0.0.1:8183)";
/// We trust our own server more than it trusts us. Its broadcasts add a
/// "userN: " prefix, so allow more than the server's 64 KiB.
const MAX_INCOMING: usize = 1024 * 1024;

/// Random bits without a crate. Each `RandomState` gets fresh keys (seeded
/// from the OS), so hashing nothing with a new one gives an unpredictable u64.
fn random_u64() -> u64 {
    RandomState::new().build_hasher().finish()
}

/// Masking (§5.3) is not encryption: the key travels in clear text right
/// before the payload. It exists so a script in a browser can't choose the
/// exact bytes that hit the wire, for example bytes that look like an HTTP
/// request to a confused proxy in the middle (cache poisoning, §10.3). For
/// that, a key only needs to be unpredictable to whoever chose the payload.
fn mask_key() -> [u8; 4] {
    (random_u64() as u32).to_be_bytes()
}

/// The write half of the socket, shared by both threads. The Mutex keeps
/// whole frames from interleaving and also guards the "close sent" flag:
/// after a close frame, nothing more may be sent (§5.5.1).
struct Connection {
    socket: TcpStream,
    closed: bool,
}

fn send(conn: &Mutex<Connection>, frame: &Frame) -> io::Result<()> {
    let mut conn = conn.lock().expect("connection lock");
    if conn.closed {
        return Ok(());
    }
    conn.closed = frame.opcode == Opcode::Close;
    frame.write_to(&mut conn.socket, Some(mask_key()))
}

/// The printer thread: shows messages, answers pings, echoes a close.
/// Returns a description of why the connection ended.
fn print_messages(reader: &mut impl Read, conn: &Mutex<Connection>) -> String {
    let mut assembler = Assembler::new(MAX_INCOMING);
    loop {
        match Frame::read_from(reader, false, MAX_INCOMING).and_then(|f| assembler.push(f)) {
            Ok(Some(Message::Text(text))) => println!("{text}"),
            Ok(Some(Message::Ping(payload))) => {
                let _ = send(conn, &Frame::new(true, Opcode::Pong, payload));
            }
            Ok(Some(Message::Close(code, reason))) => {
                // If the server started the close, `send` echoes it. If we
                // started it, this was the echo and `send` does nothing.
                let _ = send(conn, &Frame::close(code.unwrap_or(CLOSE_NORMAL), ""));
                let code = code.unwrap_or(CLOSE_NO_STATUS);
                return format!("connection closed, code {code} {reason}")
                    .trim_end()
                    .to_string();
            }
            Ok(_) => {}
            Err(WsError::Protocol(code, reason)) => {
                let _ = send(conn, &Frame::close(code, reason));
                return format!("protocol error {code}: {reason}");
            }
            Err(WsError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return "server hung up without a close frame".to_string();
            }
            Err(WsError::Io(e)) => return format!("connection lost: {e}"),
        }
    }
}

fn run(addr: &str) -> io::Result<()> {
    let socket = TcpStream::connect(addr)?;
    let mut reader = BufReader::new(socket.try_clone()?);

    let nonce = [random_u64().to_be_bytes(), random_u64().to_be_bytes()].concat();
    let key = BASE64.encode(nonce);
    (&socket).write_all(client_request(addr, &key).as_bytes())?;
    let head =
        read_head(&mut reader).map_err(|e| io::Error::other(format!("bad handshake: {e:?}")))?;
    check_response(&head, &key).map_err(io::Error::other)?;
    eprintln!("connected to ws://{addr}/ws (type a line to send it, Ctrl-D to quit)");

    let conn = Arc::new(Mutex::new(Connection {
        socket,
        closed: false,
    }));
    let printer_conn = Arc::clone(&conn);
    // Two threads, because both stdin and the socket block on read.
    let printer = thread::spawn(move || {
        let why = print_messages(&mut reader, &printer_conn);
        eprintln!("* {why}");
        // The main thread may be stuck reading stdin. Nothing left to do.
        exit(0);
    });

    for line in io::stdin().lock().lines() {
        send(&conn, &Frame::text(&line?))?;
    }
    // stdin ended: start the closing handshake. The printer exits the
    // process once the server's echo arrives.
    send(&conn, &Frame::close(CLOSE_NORMAL, "bye"))?;
    let _ = printer.join();
    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:8183".to_string());
    if addr.starts_with('-') || args.next().is_some() {
        eprintln!("{USAGE}");
        exit(2);
    }
    if let Err(e) = run(&addr) {
        eprintln!("websock-client: {e}");
        exit(1);
    }
}
