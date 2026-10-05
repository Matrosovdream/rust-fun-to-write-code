//! Integration tests over real localhost sockets, each with its own server.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(2);

struct Server {
    addr: SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// Binds port 0 and serves until `stop` fires (our stand-in for Ctrl-C).
async fn start() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(asyncchat::serve(listener, async {
        let _ = stopped.await;
    }));
    Server { addr, stop, task }
}

struct Client {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
}

impl Client {
    async fn connect(addr: SocketAddr) -> Client {
        let (read, writer) = TcpStream::connect(addr).await.unwrap().into_split();
        Client {
            lines: BufReader::new(read).lines(),
            writer,
        }
    }

    async fn send(&mut self, line: &str) {
        self.writer
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
    }

    /// The next line from the server, or `None` once it closes. Never hangs.
    async fn recv(&mut self) -> Option<String> {
        timeout(WAIT, self.lines.next_line())
            .await
            .expect("server too slow")
            .unwrap()
    }
}

#[tokio::test]
async fn two_clients_chat() {
    let server = start().await;
    let mut a = Client::connect(server.addr).await;
    assert_eq!(
        a.recv().await.unwrap(),
        "* welcome, guest1! you are in lobby. try /nick, /join, /msg, /who, /rooms, /quit"
    );
    let mut b = Client::connect(server.addr).await;
    assert!(b.recv().await.unwrap().starts_with("* welcome, guest2!"));
    assert_eq!(a.recv().await.unwrap(), "* guest2 joined lobby");

    a.send("/nick alice").await;
    assert_eq!(a.recv().await.unwrap(), "* guest1 is now alice");
    assert_eq!(b.recv().await.unwrap(), "* guest1 is now alice");

    a.send("hi").await;
    assert_eq!(b.recv().await.unwrap(), "[lobby] alice: hi");

    b.send("/msg alice psst").await;
    assert_eq!(b.recv().await.unwrap(), "[pm -> alice] psst");
    assert_eq!(a.recv().await.unwrap(), "[pm] guest2: psst");
}

#[tokio::test]
async fn long_lines_and_garbage_are_survivable() {
    let server = start().await;
    let mut a = Client::connect(server.addr).await;
    a.recv().await;

    a.send(&"x".repeat(5000)).await;
    assert_eq!(a.recv().await.unwrap(), "* line too long");
    a.writer
        .write_all(b"\xff\xfe\x00 binary\r\n")
        .await
        .unwrap();
    a.send("/who").await;
    assert_eq!(a.recv().await.unwrap(), "* users in lobby: guest1");
}

#[tokio::test]
async fn quit_says_bye_and_closes() {
    let server = start().await;
    let mut a = Client::connect(server.addr).await;
    a.recv().await;
    let mut b = Client::connect(server.addr).await;
    b.recv().await;
    a.recv().await; // "* guest2 joined lobby"

    a.send("/quit").await;
    assert_eq!(a.recv().await.unwrap(), "* bye");
    assert_eq!(a.recv().await, None);
    assert_eq!(b.recv().await.unwrap(), "* guest1 left lobby");
}

#[tokio::test]
async fn shutdown_tells_everyone_then_waits_for_them() {
    let server = start().await;
    let mut a = Client::connect(server.addr).await;
    a.recv().await;
    let mut b = Client::connect(server.addr).await;
    b.recv().await;
    a.recv().await;

    server.stop.send(()).unwrap();
    for client in [&mut a, &mut b] {
        assert_eq!(client.recv().await.unwrap(), "* server shutting down");
        assert_eq!(client.recv().await, None);
    }
    // `serve` returns once every connection task has finished...
    timeout(WAIT, server.task)
        .await
        .expect("serve didn't return")
        .unwrap();
    // ...and the listener is gone.
    assert!(TcpStream::connect(server.addr).await.is_err());
}
