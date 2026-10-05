//! Integration tests over real localhost sockets.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use binkv::{Db, MAGIC, Request, Response, frame, read_frame, serve};

/// Binds port 0 (the OS picks a free one) and serves in the background.
fn start_server(idle_timeout: Duration) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, Db::default(), idle_timeout));
    addr
}

fn start() -> SocketAddr {
    start_server(Duration::from_secs(5))
}

/// The read timeout keeps a broken server from hanging the test.
fn connect_raw(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
}

fn connect(addr: SocketAddr) -> TcpStream {
    let mut stream = connect_raw(addr);
    stream.write_all(&MAGIC).unwrap();
    stream
}

fn read_response(stream: &mut TcpStream) -> Response {
    let body = read_frame(stream).unwrap().expect("a frame, not EOF");
    Response::decode(&body).unwrap()
}

fn call(stream: &mut TcpStream, request: Request) -> Response {
    stream.write_all(&frame(&request.encode())).unwrap();
    read_response(stream)
}

fn ok(items: &[&str]) -> Response {
    Response::Ok(items.iter().map(|s| s.as_bytes().to_vec()).collect())
}

#[test]
fn put_get_del_list_over_a_socket() {
    let addr = start();
    let mut a = connect(addr);
    let key = || b"name".to_vec();

    assert_eq!(call(&mut a, Request::Ping), ok(&["PONG"]));
    assert_eq!(call(&mut a, Request::Get(key())), Response::NotFound);
    assert_eq!(call(&mut a, Request::Put(key(), b"rust".to_vec())), ok(&[]));
    assert_eq!(
        call(&mut a, Request::Put(b"lang".to_vec(), b"en".to_vec())),
        ok(&[])
    );

    // A second connection sees the same store.
    let mut b = connect(addr);
    assert_eq!(call(&mut b, Request::Get(key())), ok(&["rust"]));
    assert_eq!(call(&mut b, Request::List), ok(&["lang", "name"]));
    assert_eq!(call(&mut b, Request::Del(key())), ok(&[]));
    assert_eq!(call(&mut a, Request::Get(key())), Response::NotFound);
}

#[test]
fn raw_ping_bytes_match_the_readme() {
    // The same bytes as `printf 'BKV1\0\0\0\1\5' | nc 127.0.0.1 7100`.
    let mut stream = connect_raw(start());
    stream.write_all(b"BKV1\0\0\0\x01\x05").unwrap();
    // Half-close our side: the server sees EOF after the frame, replies,
    // and closes, so read_to_end returns.
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    assert_eq!(reply, b"\0\0\0\x07\0\0\x04PONG");
}

#[test]
fn bad_magic_gets_an_error_then_close() {
    let mut stream = connect_raw(start());
    stream.write_all(b"HELO").unwrap();
    let body = read_frame(&mut stream).unwrap().unwrap();
    assert!(matches!(Response::decode(&body), Ok(Response::Error(m)) if m.contains("magic")));
    assert_eq!(read_frame(&mut stream).unwrap(), None);
}

#[test]
fn oversized_length_gets_an_error_then_close() {
    let mut stream = connect(start());
    stream.write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
    let Response::Error(message) = read_response(&mut stream) else {
        panic!("expected an error");
    };
    assert!(message.contains("limit"), "{message}");
    assert_eq!(read_frame(&mut stream).unwrap(), None);
}

#[test]
fn malformed_frames_get_errors_and_the_connection_survives() {
    let mut stream = connect(start());
    for body in [&[0x09][..], &[0x01, 0, 10, b'x'], &[0x05, 0xaa], &[]] {
        stream.write_all(&frame(body)).unwrap();
        let response = read_response(&mut stream);
        assert!(
            matches!(response, Response::Error(_)),
            "{body:?} -> {response:?}"
        );
    }
    assert_eq!(call(&mut stream, Request::Ping), ok(&["PONG"]));
}

#[test]
fn silent_client_is_disconnected() {
    let mut stream = connect(start_server(Duration::from_millis(100)));
    // We send nothing more; the server should give up and close.
    assert_eq!(read_frame(&mut stream).unwrap(), None);
}
