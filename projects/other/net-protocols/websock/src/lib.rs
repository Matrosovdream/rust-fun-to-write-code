//! websock — a WebSocket chat server (RFC 6455).
//!
//! A WebSocket starts life as an HTTP request. The client asks to switch
//! protocols, and the server agrees with `101` (§4). From then on both sides
//! exchange *frames* on the same TCP connection, in either direction, at
//! any time:
//!
//! ```text
//! C: GET /ws HTTP/1.1
//! C: Host: 127.0.0.1:8183
//! C: Upgrade: websocket                       <- "let's switch protocols"
//! C: Connection: Upgrade
//! C: Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==   <- 16 random bytes, base64
//! C: Sec-WebSocket-Version: 13
//! C:
//! S: HTTP/1.1 101 Switching Protocols
//! S: Upgrade: websocket
//! S: Connection: Upgrade
//! S: Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=   <- base64(sha1(key + GUID))
//! S:
//! C: 81 85 37 fa 21 3d 7f 9f 4d 51 58   <- text "Hello", masked with key 37 fa 21 3d
//! S: 81 0c "user1: Hello"               <- broadcast to everyone, unmasked
//! C: 88 82 ...                          <- close 1000 (masked)
//! S: 88 02 03 e8                        <- close 1000 echoed; server closes TCP
//! ```
//!
//! Every frame starts with this header (RFC 6455 §5.2):
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-------+-+-------------+-------------------------------+
//! |F|R|R|R| opcode|M| Payload len |    Extended payload length    |
//! |I|S|S|S|  (4)  |A|     (7)     |             (16/64)           |
//! |N|V|V|V|       |S|             |   (if payload len==126/127)   |
//! | |1|2|3|       |K|             |                               |
//! +-+-+-+-+-------+-+-------------+ - - - - - - - - - - - - - - - +
//! |     Extended payload length continued, if payload len == 127  |
//! + - - - - - - - - - - - - - - - +-------------------------------+
//! |                               |Masking-key, if MASK set to 1  |
//! +-------------------------------+-------------------------------+
//! | Masking-key (continued)       |          Payload Data         |
//! +-------------------------------- - - - - - - - - - - - - - - - +
//! :                     Payload Data continued ...                :
//! + - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - - +
//! |                     Payload Data continued ...                |
//! +---------------------------------------------------------------+
//! ```
//!
//! - **FIN**: this is the last fragment of a message.
//! - **RSV1-3**: reserved for extensions. Must be 0 here.
//! - **opcode**: 0x0 continuation, 0x1 text, 0x2 binary, 0x8 close, 0x9 ping,
//!   0xA pong. Opcodes from 0x8 up are control frames: at most 125 bytes,
//!   never fragmented (§5.5).
//! - **MASK + masking key**: every client-to-server frame is XORed with a
//!   random 4-byte key. Server frames are never masked (§5.3).
//! - **Payload len**: 0-125 is the length itself. 126 means the next 16 bits
//!   hold it, and 127 means the next 64 bits do (big-endian).
//!
//! Errors close the connection with a status code (§7.4.1): 1002 for a
//! protocol violation, 1007 for text that isn't UTF-8, 1009 for a message
//! over [`MAX_MESSAGE`].
//!
//! Inside the server, each client gets a reader thread (its connection
//! thread) and a writer thread. A hub thread owns the client list and fans
//! text out to every writer through per-client `mpsc` channels (outboxes).

pub mod frame;
pub mod handshake;

use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use frame::{Assembler, CLOSE_NO_STATUS, CLOSE_NORMAL, Frame, Message, Opcode, WsError};
use handshake::{HeadError, accept_response, check_upgrade, read_head};

/// The chat page, compiled into the binary.
pub const INDEX_HTML: &str = include_str!("../static/index.html");
/// Largest message (after reassembly) we accept from a client.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// How long a client may take to send its HTTP request.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a write to a client that stopped reading may block.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// A quiet chat is normal, so there's no read timeout once upgraded.
/// Instead the writer pings an idle client. If the peer has vanished, that
/// write eventually fails, and the connection is torn down.
const PING_EVERY: Duration = Duration::from_secs(30);

/// What connection threads tell the hub.
enum HubEvent {
    /// A new client, with the Sender half of its outbox.
    Join(u64, Sender<Frame>),
    Leave(u64),
    Say(u64, String),
}

/// The hub thread owns the client list. Nobody else touches it, so it needs
/// no Mutex (the actor pattern from linechat). It turns every event into a
/// line of text and sends that to every client's writer.
fn run_hub(events: Receiver<HubEvent>) {
    let mut clients: Vec<(u64, Sender<Frame>)> = Vec::new();
    for event in events {
        let text = match event {
            HubEvent::Join(id, outbox) => {
                clients.push((id, outbox));
                format!("* user{id} joined ({} online)", clients.len())
            }
            HubEvent::Leave(id) => {
                clients.retain(|(client, _)| *client != id);
                format!("* user{id} left")
            }
            HubEvent::Say(id, text) => format!("user{id}: {text}"),
        };
        // A failed send means that client's writer thread is gone: prune it.
        clients.retain(|(_, outbox)| outbox.send(Frame::text(&text)).is_ok());
    }
}

/// The writer thread: the only code that writes to the socket after the
/// handshake. Frames from the hub and from our own reader (pongs, close) all
/// funnel through this client's outbox channel, so two threads can never
/// interleave bytes mid-frame.
fn write_frames(mut socket: TcpStream, outbox: Receiver<Frame>) {
    loop {
        let frame = match outbox.recv_timeout(PING_EVERY) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) => Frame::new(true, Opcode::Ping, Vec::new()),
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Nothing may follow a close frame (§5.5.1).
        if frame.write_to(&mut socket, None).is_err() || frame.opcode == Opcode::Close {
            break;
        }
    }
    // Shutting down both halves also wakes the reader if it's blocked in read.
    let _ = socket.shutdown(Shutdown::Both);
}

/// Reads frames until the conversation ends, and returns why it ended.
fn read_frames(
    reader: &mut impl Read,
    id: u64,
    hub: &Sender<HubEvent>,
    outbox: &Sender<Frame>,
) -> String {
    let mut assembler = Assembler::new(MAX_MESSAGE);
    loop {
        // `and_then` chains the two fallible steps: read a frame, then feed it.
        match Frame::read_from(reader, true, MAX_MESSAGE).and_then(|f| assembler.push(f)) {
            Ok(Some(Message::Text(text))) => {
                let _ = hub.send(HubEvent::Say(id, text));
            }
            Ok(Some(Message::Ping(payload))) => {
                let _ = outbox.send(Frame::new(true, Opcode::Pong, payload));
            }
            Ok(Some(Message::Close(code, _))) => {
                // Echo the close (§5.5.1). The writer then closes the TCP connection.
                let _ = outbox.send(Frame::close(code.unwrap_or(CLOSE_NORMAL), ""));
                return format!("closed by client, code {}", code.unwrap_or(CLOSE_NO_STATUS));
            }
            // Mid-fragment, an unsolicited pong, or binary (this chat is text-only).
            Ok(_) => {}
            Err(WsError::Protocol(code, reason)) => {
                let _ = outbox.send(Frame::close(code, reason));
                return format!("protocol error {code}: {reason}");
            }
            Err(WsError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return "hung up without a close frame".to_string();
            }
            Err(WsError::Io(e)) => return format!("connection lost: {e}"),
        }
    }
}

/// Runs an upgraded connection: answers 101, then reads frames on this
/// thread while a second thread writes.
fn run_session(
    mut reader: BufReader<TcpStream>,
    id: u64,
    key: &str,
    hub: &Sender<HubEvent>,
) -> io::Result<String> {
    let mut socket = reader.get_ref().try_clone()?;
    socket.set_read_timeout(None)?;
    let (outbox, outbox_rx) = mpsc::channel();
    // Join *before* sending the 101. Once a client sees the 101 it may talk
    // right away, and its first message must find everyone already joined.
    hub.send(HubEvent::Join(id, outbox.clone()))
        .expect("hub thread runs forever");
    socket.write_all(accept_response(key).as_bytes())?;
    let writer_thread = thread::spawn(move || write_frames(socket, outbox_rx));

    // The BufReader may already hold the client's first frames (sent right
    // behind the handshake), so keep reading through it.
    let why = read_frames(&mut reader, id, hub, &outbox);
    let _ = hub.send(HubEvent::Leave(id));
    // With our Sender and the hub's copy both gone, the outbox closes: the
    // writer drains what's queued (maybe a close frame) and exits.
    drop(outbox);
    let _ = writer_thread.join();
    Ok(why)
}

/// A complete plain-HTTP response, for everything that isn't an upgrade.
fn reply(out: &mut impl Write, status: &str, content_type: &str, body: &str) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    out.write_all(response.as_bytes())
}

fn handle(socket: TcpStream, id: u64, hub: Sender<HubEvent>) -> io::Result<()> {
    let peer = socket.peer_addr()?;
    socket.set_read_timeout(Some(HEAD_TIMEOUT))?;
    socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut reader = BufReader::new(socket.try_clone()?);
    let mut out = socket;

    let head = match read_head(&mut reader) {
        Ok(head) => head,
        Err(HeadError::Io(_)) => return Ok(()), // silent or gone: nobody to answer
        Err(e) => {
            eprintln!("{peer} bad request: {e:?}");
            return reply(&mut out, "400 Bad Request", "text/plain", "bad request\n");
        }
    };
    eprintln!("{peer} {}", head.first_line);

    let mut words = head.first_line.split(' ');
    match (words.next(), words.next()) {
        (Some("GET"), Some("/")) => {
            reply(&mut out, "200 OK", "text/html; charset=utf-8", INDEX_HTML)
        }
        // check_upgrade insists on GET itself, with a helpful error.
        (_, Some("/ws")) => match check_upgrade(&head) {
            Ok(key) => {
                let why = run_session(reader, id, key, &hub)?;
                eprintln!("{peer} user{id} left: {why}");
                Ok(())
            }
            Err(rejection) => out.write_all(rejection.response().as_bytes()),
        },
        _ => reply(&mut out, "404 Not Found", "text/plain", "not found\n"),
    }
}

/// Starts the hub thread, then accepts connections, one thread each.
/// Connection numbers double as user names (`user7`).
pub fn serve(listener: TcpListener) {
    let (hub, events) = mpsc::channel();
    thread::spawn(move || run_hub(events));
    for (id, socket) in (1..).zip(listener.incoming()) {
        match socket {
            Ok(socket) => {
                let hub = hub.clone();
                thread::spawn(move || {
                    if let Err(e) = handle(socket, id, hub) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}
