//! Integration tests over real UDP sockets on localhost.

use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::Duration;

use dnsd::wire::{FORMERR, NOERROR};
use dnsd::{Message, RData, Zone, serve};

/// The real query `dig @127.0.0.1 -p 10054 hello.test` sends, EDNS OPT
/// record included.
const DIG_QUERY: [u8; 39] = [
    0xd9, 0xba, 0x01, 0x20, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, // header
    0x05, b'h', b'e', b'l', b'l', b'o', 0x04, b't', b'e', b's', b't', 0x00, // name
    0x00, 0x01, 0x00, 0x01, // QTYPE A, QCLASS IN
    0x00, 0x00, 0x29, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // OPT
];

/// Binds port 0 (the OS picks a free one) and serves the shipped zone file
/// in the background.
fn start() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    let zone = Zone::parse(include_str!("../zone.txt")).unwrap();
    thread::spawn(move || serve(socket, &zone));
    addr
}

fn client(timeout_ms: u64) -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let timeout = Duration::from_millis(timeout_ms);
    socket.set_read_timeout(Some(timeout)).unwrap();
    socket
}

fn ask(socket: &UdpSocket, server: SocketAddr, query: &[u8]) -> Message {
    socket.send_to(query, server).unwrap();
    let mut buf = [0u8; 512];
    let (n, _) = socket.recv_from(&mut buf).unwrap();
    Message::parse(&buf[..n]).unwrap()
}

#[test]
fn answers_a_real_dig_query() {
    let server = start();
    let reply = ask(&client(2000), server, &DIG_QUERY);

    assert_eq!(reply.header.id, 0xd9ba);
    assert!(reply.header.qr && reply.header.aa);
    assert_eq!(reply.header.rcode, NOERROR);
    let addrs: Vec<_> = reply.answers.iter().map(|r| r.data.clone()).collect();
    assert_eq!(
        addrs,
        [
            RData::A("192.0.2.1".parse().unwrap()),
            RData::A("192.0.2.2".parse().unwrap())
        ]
    );
}

#[test]
fn survives_garbage() {
    let server = start();
    let socket = client(300);

    // Too short to have an ID: silently dropped.
    socket.send_to(b"\x01", server).unwrap();
    assert!(socket.recv_from(&mut [0u8; 512]).is_err());

    // A header followed by a pointer loop: FORMERR with the same ID.
    let reply = ask(
        &socket,
        server,
        b"\xab\xcd\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\xc0\x0c",
    );
    assert_eq!((reply.header.id, reply.header.rcode), (0xabcd, FORMERR));

    // Still serving.
    assert_eq!(ask(&socket, server, &DIG_QUERY).answers.len(), 2);
}
