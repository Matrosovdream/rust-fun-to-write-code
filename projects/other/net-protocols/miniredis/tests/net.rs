//! Integration tests over real localhost sockets.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use miniredis::{Db, MAX_BUFFER, serve};

/// Binds port 0 (the OS picks a free one) and serves in a background thread.
fn start_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let db = Arc::new(Mutex::new(Db::new()));
    thread::spawn(move || serve(listener, db, Duration::from_secs(5)));
    addr
}

fn connect(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    // A bug should fail the test, not hang it.
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
}

/// Reads exactly as many bytes as `expected` has, and compares.
fn expect(stream: &mut TcpStream, expected: &str) {
    let mut got = vec![0; expected.len()];
    stream.read_exact(&mut got).unwrap();
    assert_eq!(String::from_utf8_lossy(&got), expected);
}

fn expect_closed(stream: &mut TcpStream) {
    let mut rest = Vec::new();
    assert_eq!(stream.read_to_end(&mut rest).unwrap(), 0);
}

#[test]
fn pipelined_requests_are_answered_in_order() {
    let addr = start_server();
    let mut c = connect(addr);
    // One write: inline commands (CRLF and bare LF) mixed with a RESP array.
    c.write_all(b"PING\r\n*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$5\r\nhello\r\nGET k\r\nINCR n\nINCR n\r\nGET nope\r\n")
        .unwrap();
    expect(&mut c, "+PONG\r\n+OK\r\n$5\r\nhello\r\n:1\r\n:2\r\n$-1\r\n");

    // Another connection sees the same data.
    let mut other = connect(addr);
    other.write_all(b"EXISTS k n nope\r\n").unwrap();
    expect(&mut other, ":2\r\n");
}

#[test]
fn a_request_split_across_writes_waits_for_the_rest() {
    let addr = start_server();
    let mut c = connect(addr);
    // The ECHO request is cut in the middle: PING is answered, ECHO waits.
    c.write_all(b"PING\r\n*2\r\n$4\r\nEC").unwrap();
    expect(&mut c, "+PONG\r\n");
    c.write_all(b"HO\r\n$2\r\nhi\r\n").unwrap();
    expect(&mut c, "$2\r\nhi\r\n");
}

#[test]
fn keys_expire_after_px() {
    let addr = start_server();
    let mut c = connect(addr);
    c.write_all(b"SET short 1 PX 20\r\nSET long 1 PX 10000\r\nTTL long\r\n")
        .unwrap();
    expect(&mut c, "+OK\r\n+OK\r\n:10\r\n");
    thread::sleep(Duration::from_millis(50));
    c.write_all(b"GET short\r\nDBSIZE\r\n").unwrap();
    expect(&mut c, "$-1\r\n:1\r\n");
}

#[test]
fn garbage_gets_an_error_then_the_connection_closes() {
    let addr = start_server();
    let mut c = connect(addr);
    c.write_all(b"PING\r\n*1\r\n$abc\r\n").unwrap();
    expect(&mut c, "+PONG\r\n-ERR Protocol error: invalid integer\r\n");
    expect_closed(&mut c);
}

#[test]
fn oversized_requests_are_refused() {
    let addr = start_server();

    // A huge declared bulk length is refused before any of the data arrives.
    let mut c = connect(addr);
    c.write_all(b"*2\r\n$4\r\nECHO\r\n$999999999\r\n").unwrap();
    expect(&mut c, "-ERR Protocol error: invalid bulk length\r\n");
    expect_closed(&mut c);

    // A line that never ends is refused once it outgrows the buffer limit.
    let mut c = connect(addr);
    c.write_all(&vec![b'a'; MAX_BUFFER + 1]).unwrap();
    expect(&mut c, "-ERR Protocol error: request too large\r\n");
    expect_closed(&mut c);
}
