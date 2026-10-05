//! End-to-end tests inside one process: an echo server stands in for the
//! private service, and the relay and agent run as tasks on port-0 sockets.

use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tunnel::{Config, run_agent, run_relay};

const WAIT: Duration = Duration::from_secs(3);

fn fast_config() -> Config {
    Config {
        token: "test".to_string(),
        pending_timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(1),
        heartbeat: Duration::from_millis(50),
        backoff_min: Duration::from_millis(20),
        backoff_max: Duration::from_millis(100),
    }
}

/// The "private service": echoes every byte back, then closes on EOF.
async fn start_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut from, mut to) = stream.split();
                let _ = tokio::io::copy(&mut from, &mut to).await;
            });
        }
    });
    addr
}

struct Relay {
    control: SocketAddr,
    public: SocketAddr,
    task: JoinHandle<()>,
}

async fn start_relay(control: &str, public: &str, config: Config) -> Relay {
    let control = TcpListener::bind(control).await.unwrap();
    let public = TcpListener::bind(public).await.unwrap();
    let (c, p) = (control.local_addr().unwrap(), public.local_addr().unwrap());
    let task = tokio::spawn(run_relay(control, public, config));
    Relay {
        control: c,
        public: p,
        task,
    }
}

fn start_agent(relay: SocketAddr, target: SocketAddr, config: Config) -> JoinHandle<io::Error> {
    tokio::spawn(async move { run_agent(&relay.to_string(), &target.to_string(), &config).await })
}

/// Sends `payload` through the public port, half-closes, and returns
/// everything that comes back.
async fn round_trip(public: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(public).await.unwrap();
    stream.write_all(payload).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut back = Vec::new();
    timeout(WAIT, stream.read_to_end(&mut back))
        .await
        .expect("tunnel too slow")
        .unwrap();
    back
}

async fn read_line(reader: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    timeout(WAIT, reader.read_line(&mut line))
        .await
        .expect("no reply")
        .unwrap();
    line
}

/// The peer closed: EOF, or a reset if it closed with our bytes still unread.
async fn assert_closed(reader: &mut BufReader<TcpStream>) {
    let mut line = String::new();
    match timeout(WAIT, reader.read_line(&mut line))
        .await
        .expect("still open")
    {
        Ok(n) => assert_eq!(n, 0, "expected EOF, got {line:?}"),
        Err(e) => assert_eq!(e.kind(), io::ErrorKind::ConnectionReset),
    }
}

#[tokio::test]
async fn bytes_go_through_the_tunnel_and_back() {
    let echo = start_echo().await;
    let relay = start_relay("127.0.0.1:0", "127.0.0.1:0", fast_config()).await;
    let _agent = start_agent(relay.control, echo, fast_config());

    assert_eq!(
        round_trip(relay.public, b"hello through the tunnel").await,
        b"hello through the tunnel"
    );
}

#[tokio::test]
async fn concurrent_connections_stay_separate() {
    let echo = start_echo().await;
    let relay = start_relay("127.0.0.1:0", "127.0.0.1:0", fast_config()).await;
    let _agent = start_agent(relay.control, echo, fast_config());

    let mut clients = tokio::task::JoinSet::new();
    for i in 0..20u8 {
        // Different sizes, up to 200 KB, so copies interleave for real.
        let payload = vec![i; 1 + usize::from(i) * 10_000];
        clients.spawn(async move { (round_trip(relay.public, &payload).await, payload) });
    }
    while let Some(result) = clients.join_next().await {
        let (back, sent) = result.unwrap();
        assert!(
            back == sent,
            "a client got {} bytes instead of its own {}",
            back.len(),
            sent.len()
        );
    }
}

#[tokio::test]
async fn public_client_without_agent_times_out() {
    let config = Config {
        pending_timeout: Duration::from_millis(100),
        ..fast_config()
    };
    let relay = start_relay("127.0.0.1:0", "127.0.0.1:0", config).await;

    // Send nothing: closing a socket with unread input would turn the
    // relay's close into a reset, which is fine for curl but noisy here.
    let mut client = TcpStream::connect(relay.public).await.unwrap();
    let started = Instant::now();
    let mut rest = Vec::new();
    let n = timeout(WAIT, client.read_to_end(&mut rest))
        .await
        .expect("never timed out");
    assert_eq!(n.unwrap(), 0);
    assert!(started.elapsed() >= Duration::from_millis(100));
}

#[tokio::test]
async fn bad_tokens_and_junk_are_turned_away() {
    let relay = start_relay("127.0.0.1:0", "127.0.0.1:0", fast_config()).await;

    let mut conn = BufReader::new(TcpStream::connect(relay.control).await.unwrap());
    conn.get_mut().write_all(b"HELLO wrong\n").await.unwrap();
    assert_eq!(read_line(&mut conn).await, "ERR bad token\n");
    assert_eq!(
        read_line(&mut conn).await,
        "",
        "relay should close after ERR"
    );

    let junk: [&[u8]; 3] = [b"GET / HTTP/1.1\r\n", b"DATA 999\n", &[b'x'; 5000]];
    for first_line in junk {
        let mut conn = BufReader::new(TcpStream::connect(relay.control).await.unwrap());
        // The relay may close before reading everything, so ignore write errors.
        let _ = conn.get_mut().write_all(first_line).await;
        assert_closed(&mut conn).await;
    }

    // The real agent gives up for good on a refusal instead of retrying.
    let echo = start_echo().await;
    let wrong = Config {
        token: "wrong".to_string(),
        ..fast_config()
    };
    let err = timeout(WAIT, start_agent(relay.control, echo, wrong))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
}

#[tokio::test]
async fn relay_answers_pings_and_drops_a_silent_agent() {
    let relay = start_relay("127.0.0.1:0", "127.0.0.1:0", fast_config()).await;
    let mut agent = BufReader::new(TcpStream::connect(relay.control).await.unwrap());
    agent.get_mut().write_all(b"HELLO test\n").await.unwrap();
    assert_eq!(read_line(&mut agent).await, "OK\n");
    agent.get_mut().write_all(b"PING\n").await.unwrap();
    assert_eq!(read_line(&mut agent).await, "PONG\n");

    // Now say nothing: after 3 heartbeats (150 ms) the relay hangs up.
    assert_eq!(read_line(&mut agent).await, "");
}

#[tokio::test]
async fn agent_reconnects_when_the_relay_goes_quiet() {
    // A fake relay that says OK and then never says anything again.
    let fake = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let _agent = start_agent(
        fake.local_addr().unwrap(),
        start_echo().await,
        fast_config(),
    );
    let mut sessions = Vec::new();
    for _ in 0..2 {
        let (stream, _) = timeout(WAIT, fake.accept())
            .await
            .expect("agent didn't (re)connect")
            .unwrap();
        let mut stream = BufReader::new(stream);
        assert_eq!(read_line(&mut stream).await, "HELLO test\n");
        stream.get_mut().write_all(b"OK\n").await.unwrap();
        sessions.push(stream); // keep it open, but silent
    }
}

#[tokio::test]
async fn agent_reconnects_after_relay_restart() {
    let echo = start_echo().await;
    let relay = start_relay("127.0.0.1:0", "127.0.0.1:0", fast_config()).await;
    let _agent = start_agent(relay.control, echo, fast_config());
    assert_eq!(round_trip(relay.public, b"before").await, b"before");

    // Kill the relay: aborting drops its JoinSet, which closes every socket.
    relay.task.abort();
    let _ = relay.task.await;

    // Start a new relay on the same ports. The agent finds it by backing off
    // and retrying. Our client may arrive first: then it waits in `pending`
    // until the agent's HELLO.
    let (control, public) = (relay.control.to_string(), relay.public.to_string());
    let relay = start_relay(&control, &public, fast_config()).await;
    assert_eq!(round_trip(relay.public, b"after").await, b"after");
}
