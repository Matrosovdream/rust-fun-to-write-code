//! Integration tests over real localhost sockets.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

/// Binds port 0 (the OS picks a free one) and serves in a background thread.
fn start_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || linechat::serve(listener));
    addr
}

struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    /// Connects and swallows the welcome line.
    fn connect(addr: SocketAddr) -> Client {
        let stream = TcpStream::connect(addr).unwrap();
        // A bug should fail the test, not hang it.
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut client = Client {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
        };
        assert!(client.recv().starts_with("* welcome, guest"));
        client
    }

    fn send(&mut self, line: &str) {
        writeln!(self.writer, "{line}").unwrap();
    }

    fn recv(&mut self) -> String {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        line.trim_end().to_string()
    }
}

#[test]
fn two_clients_see_each_other() {
    let addr = start_server();
    let mut a = Client::connect(addr);
    let mut b = Client::connect(addr);
    assert_eq!(a.recv(), "* guest2 joined lobby");

    a.send("/nick alice");
    assert_eq!(a.recv(), "* guest1 is now alice");
    assert_eq!(b.recv(), "* guest1 is now alice");

    b.send("hello alice");
    assert_eq!(a.recv(), "[lobby] guest2: hello alice");

    a.send("/msg guest2 psst");
    assert_eq!(a.recv(), "[pm -> guest2] psst");
    assert_eq!(b.recv(), "[pm] alice: psst");

    // /quit: bye, then the server closes our socket and tells the room.
    a.send("/quit");
    assert_eq!(a.recv(), "* bye");
    assert_eq!(a.recv(), ""); // EOF
    assert_eq!(b.recv(), "* alice left lobby");
}

#[test]
fn disconnect_is_announced_and_long_lines_rejected() {
    let addr = start_server();
    let mut a = Client::connect(addr);
    let b = Client::connect(addr);
    assert_eq!(a.recv(), "* guest2 joined lobby");

    a.send(&"x".repeat(5000));
    assert_eq!(a.recv(), "* line too long");
    a.send("/who"); // the connection still works afterwards
    assert_eq!(a.recv(), "* users in lobby: guest1, guest2");

    drop(b); // hang up without /quit
    assert_eq!(a.recv(), "* guest2 left lobby");
}
