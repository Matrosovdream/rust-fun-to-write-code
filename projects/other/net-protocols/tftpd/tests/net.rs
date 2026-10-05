//! Integration tests over real UDP sockets on localhost. The test plays the
//! client, using the library's own `Packet` codec.

use std::fs;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::thread;
use std::time::Duration;

use tftpd::{BLOCK_SIZE, Config, ErrorCode, Packet, serve};

/// Short timeout to keep tests fast, but many tries so a busy test machine
/// can't make a transfer give up by accident.
const FAST: Config = Config {
    timeout: Duration::from_millis(50),
    tries: 20,
};

/// Binds port 0 (the OS picks a free one) and serves `root` in the
/// background.
fn start(root: &Path, config: Config) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    let root = root.to_path_buf();
    thread::spawn(move || serve(socket, root, config));
    addr
}

fn client() -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    socket
}

fn send(socket: &UdpSocket, to: SocketAddr, packet: Packet) {
    socket.send_to(&packet.encode(), to).unwrap();
}

fn recv(socket: &UdpSocket) -> (Packet, SocketAddr) {
    let mut buf = [0u8; 1024];
    let (n, from) = socket.recv_from(&mut buf).unwrap();
    (Packet::parse(&buf[..n]).unwrap(), from)
}

fn rrq(name: &str) -> Packet {
    Packet::Rrq {
        filename: name.into(),
        mode: "octet".into(),
    }
}

fn wrq(name: &str) -> Packet {
    Packet::Wrq {
        filename: name.into(),
        mode: "octet".into(),
    }
}

/// Sends a request and returns the error code the server answers with.
fn error_code(server: SocketAddr, request: Packet) -> u16 {
    let socket = client();
    send(&socket, server, request);
    match recv(&socket).0 {
        Packet::Error { code, .. } => code,
        other => panic!("expected ERROR, got {other:?}"),
    }
}

/// Downloads a file; returns its bytes and how many blocks it took.
fn download(server: SocketAddr, name: &str) -> (Vec<u8>, u16) {
    let socket = client();
    send(&socket, server, rrq(name));
    let mut file = Vec::new();
    let mut block = 0u16;
    loop {
        let (packet, tid) = recv(&socket);
        let Packet::Data { block: got, data } = packet else {
            panic!("expected DATA, got {packet:?}");
        };
        // A late duplicate of a block we already have: just skip it.
        if got == block {
            continue;
        }
        assert_eq!(got, block + 1);
        block = got;
        file.extend(&data);
        send(&socket, tid, Packet::Ack { block });
        if data.len() < BLOCK_SIZE {
            return (file, block);
        }
    }
}

#[test]
fn downloads_files_of_awkward_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path(), FAST);
    // (size, blocks): an exact multiple of 512 needs an extra empty block.
    for (size, blocks) in [(0, 1), (511, 1), (512, 2), (1025, 3)] {
        let content: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let name = format!("file{size}.bin");
        fs::write(dir.path().join(&name), &content).unwrap();

        assert_eq!(download(server, &name), (content, blocks), "size {size}");
    }
}

#[test]
fn upload_is_renamed_into_place() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path(), FAST);
    let content: Vec<u8> = (0..1300).map(|i| (i % 7) as u8).collect();

    let socket = client();
    send(&socket, server, wrq("up.bin"));
    let (ack, tid) = recv(&socket);
    assert_eq!(ack, Packet::Ack { block: 0 });

    for (i, chunk) in content.chunks(BLOCK_SIZE).enumerate() {
        let block = i as u16 + 1;
        let data = chunk.to_vec();
        send(&socket, tid, Packet::Data { block, data });
        // Skip any duplicate ACK of the previous block.
        while recv(&socket).0 != (Packet::Ack { block }) {}
    }

    // 1300 bytes is 512 + 512 + 276: the final ACK means the file is in place.
    assert_eq!(fs::read(dir.path().join("up.bin")).unwrap(), content);
    let names: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(names.len(), 1, "temporary file left behind");
}

#[test]
fn refuses_bad_requests_and_keeps_going() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("taken.txt"), "still here").unwrap();
    let server = start(dir.path(), FAST);

    let not_found = ErrorCode::FileNotFound as u16;
    let access = ErrorCode::AccessViolation as u16;
    assert_eq!(error_code(server, rrq("missing.txt")), not_found);
    assert_eq!(error_code(server, rrq("../secret")), access);
    assert_eq!(error_code(server, rrq("/etc/passwd")), access);
    assert_eq!(
        error_code(server, wrq("taken.txt")),
        ErrorCode::FileExists as u16
    );

    // Illegal operations: an unsupported mode, a packet that isn't a
    // request, and plain garbage.
    let illegal = ErrorCode::IllegalOperation as u16;
    let mail = Packet::Rrq {
        filename: "taken.txt".into(),
        mode: "mail".into(),
    };
    assert_eq!(error_code(server, mail), illegal);
    assert_eq!(error_code(server, Packet::Ack { block: 1 }), illegal);
    let socket = client();
    for garbage in [&b""[..], b"\x00", b"\x00\x09junk", b"\x00\x01no-terminator"] {
        socket.send_to(garbage, server).unwrap();
        let (reply, _) = recv(&socket);
        assert!(matches!(reply, Packet::Error { code: 4, .. }), "{reply:?}");
    }

    assert_eq!(download(server, "taken.txt").0, b"still here");
}

#[test]
fn survives_a_lost_ack_and_a_stranger() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("f.bin"), [9u8; 600]).unwrap();
    let server = start(dir.path(), FAST);

    let socket = client();
    send(&socket, server, rrq("f.bin"));
    let (first, tid) = recv(&socket);
    // Pretend our ACK got lost: say nothing, and the same block comes again.
    assert_eq!(recv(&socket).0, first);

    // A packet from another port gets ERROR 5 and leaves the transfer alone.
    let stranger = client();
    send(&stranger, tid, Packet::Ack { block: 1 });
    let (reply, _) = recv(&stranger);
    assert!(matches!(reply, Packet::Error { code: 5, .. }), "{reply:?}");

    send(&socket, tid, Packet::Ack { block: 1 });
    // Skip any further copies of block 1 until block 2 arrives.
    loop {
        if let (Packet::Data { block: 2, data }, _) = recv(&socket) {
            assert_eq!(data.len(), 600 - BLOCK_SIZE);
            break;
        }
    }
    send(&socket, tid, Packet::Ack { block: 2 });
}

#[test]
fn gives_up_after_the_configured_tries() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("f.bin"), [1u8; 10]).unwrap();
    let config = Config {
        timeout: Duration::from_millis(30),
        tries: 3,
    };
    let server = start(dir.path(), config);

    let socket = client();
    let silence = Duration::from_millis(300);
    socket.set_read_timeout(Some(silence)).unwrap();
    send(&socket, server, rrq("f.bin"));
    // Never ACK: count copies of DATA 1 until the server falls silent.
    let mut copies = 0;
    while socket.recv_from(&mut [0u8; 1024]).is_ok() {
        copies += 1;
    }
    assert_eq!(copies, 3);
}
