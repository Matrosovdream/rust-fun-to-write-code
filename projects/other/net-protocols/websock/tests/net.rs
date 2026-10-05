//! Integration tests over real localhost sockets.

use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use websock::frame::{Frame, Opcode};
use websock::handshake::{check_response, client_request, read_head};
use websock::serve;

const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const MASK: Option<[u8; 4]> = Some([1, 2, 3, 4]);

fn start() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener));
    addr
}

/// Sends `raw` and returns everything the server says before closing.
fn http(addr: SocketAddr, raw: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// A test client: does the handshake, then returns (frame reader, writer).
fn join(addr: SocketAddr) -> (BufReader<TcpStream>, TcpStream) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(client_request(&addr.to_string(), KEY).as_bytes())
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    check_response(&read_head(&mut reader).unwrap(), KEY).unwrap();
    (reader, stream)
}

fn send(writer: &mut TcpStream, frame: Frame) {
    frame.write_to(writer, MASK).unwrap();
}

fn next_frame(reader: &mut BufReader<TcpStream>) -> Frame {
    Frame::read_from(reader, false, 1 << 20).unwrap()
}

/// Reads frames until a text frame ends with `suffix`, skipping join notices.
fn wait_for_text(reader: &mut BufReader<TcpStream>, suffix: &str) -> String {
    loop {
        let frame = next_frame(reader);
        let text = String::from_utf8(frame.payload).unwrap();
        if frame.opcode == Opcode::Text && text.ends_with(suffix) {
            return text;
        }
    }
}

/// Reads frames until a close arrives and returns its status code.
fn wait_for_close(reader: &mut BufReader<TcpStream>) -> u16 {
    loop {
        let frame = next_frame(reader);
        if frame.opcode == Opcode::Close {
            return u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
        }
    }
}

#[test]
fn handshake_returns_the_rfc_accept_key() {
    let addr = start();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(client_request("t", KEY).as_bytes())
        .unwrap();
    let head = read_head(&mut BufReader::new(stream)).unwrap();
    assert_eq!(head.first_line, "HTTP/1.1 101 Switching Protocols");
    assert_eq!(
        head.header("Sec-WebSocket-Accept"),
        Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    );
}

#[test]
fn serves_the_page_and_refuses_bad_upgrades() {
    let addr = start();
    let page = http(addr, "GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert!(page.starts_with("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n"));
    assert!(page.contains("new WebSocket("));

    let plain_get = http(addr, "GET /ws HTTP/1.1\r\nHost: t\r\n\r\n");
    assert!(plain_get.starts_with("HTTP/1.1 400 "), "{plain_get}");
    let old_version = client_request("t", KEY).replace("Version: 13", "Version: 8");
    let old_version = http(addr, &old_version);
    assert!(
        old_version.starts_with("HTTP/1.1 426 ")
            && old_version.contains("Sec-WebSocket-Version: 13")
    );
    assert!(http(addr, "GET /nope HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404 "));
}

#[test]
fn two_clients_chat_including_fragmented_messages() {
    let addr = start();
    let (mut alice_in, mut alice_out) = join(addr);
    let (mut bob_in, mut bob_out) = join(addr);

    send(&mut alice_out, Frame::text("hello bob"));
    assert!(wait_for_text(&mut bob_in, ": hello bob").starts_with("user"));

    // Bob sends "hi alice" in two fragments with a ping in between.
    send(&mut bob_out, Frame::new(false, Opcode::Text, "hi "));
    send(&mut bob_out, Frame::new(true, Opcode::Ping, "still here"));
    send(
        &mut bob_out,
        Frame::new(true, Opcode::Continuation, "alice"),
    );
    wait_for_text(&mut alice_in, ": hi alice");
    // The ping got a pong with the same payload.
    loop {
        let frame = next_frame(&mut bob_in);
        if frame.opcode == Opcode::Pong {
            assert_eq!(frame.payload, b"still here");
            break;
        }
    }
}

#[test]
fn close_is_echoed_and_then_the_server_hangs_up() {
    let addr = start();
    let (mut reader, mut writer) = join(addr);
    send(&mut writer, Frame::close(1000, "bye"));
    assert_eq!(wait_for_close(&mut reader), 1000);
    assert_eq!(reader.read(&mut [0; 16]).unwrap(), 0); // EOF
}

#[test]
fn protocol_violations_close_with_the_right_code() {
    let addr = start();
    // Each case is consumed whole by the server before it fails. Leftover
    // unread bytes would make its close() send a reset that can eat the reply.
    let cases = [
        (Frame::new(true, Opcode::Text, "hi"), None, 1002), // not masked
        (Frame::new(true, Opcode::Text, [0xC3, 0x28]), MASK, 1007), // invalid UTF-8
    ];
    for (frame, mask, code) in cases {
        let (mut reader, mut writer) = join(addr);
        frame.write_to(&mut writer, mask).unwrap();
        assert_eq!(wait_for_close(&mut reader), code);
    }
    // A header claiming 65537 bytes is refused before any payload is sent.
    let (mut reader, mut writer) = join(addr);
    writer
        .write_all(&[0x82, 0xFF, 0, 0, 0, 0, 0, 1, 0, 1])
        .unwrap();
    assert_eq!(wait_for_close(&mut reader), 1009);
}
