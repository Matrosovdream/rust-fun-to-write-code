//! Integration tests over real localhost UDP sockets.

use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::Duration;

use udpping::{MAX_DATAGRAM, Outcome, PingConfig, Probe, ServerConfig, ping, serve};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Binds port 0 (the OS picks a free one) and serves in the background.
/// A fixed seed keeps the drop decisions repeatable.
fn start_server(drop: f64, delay: Duration) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    thread::spawn(move || {
        serve(
            socket,
            ServerConfig {
                drop,
                delay,
                seed: 42,
            },
        )
    });
    addr
}

fn client_socket() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").unwrap()
}

/// Runs a ping session and returns every outcome in order.
fn run(server: SocketAddr, count: u32, timeout: Duration) -> (Vec<Outcome>, udpping::Stats) {
    let config = PingConfig {
        count,
        interval: ms(5),
        timeout,
    };
    let mut outcomes = Vec::new();
    let stats = ping(&client_socket(), server, &config, |o| outcomes.push(o)).unwrap();
    (outcomes, stats)
}

#[test]
fn every_probe_comes_back() {
    let (outcomes, stats) = run(start_server(0.0, Duration::ZERO), 3, ms(1000));
    let seqs: Vec<u32> = outcomes
        .iter()
        .map(|o| match o {
            Outcome::Reply { seq, .. } => *seq,
            Outcome::Timeout { seq } => panic!("probe {seq} timed out"),
        })
        .collect();
    assert_eq!(seqs, [0, 1, 2]);
    assert_eq!((stats.sent, stats.received), (3, 3));
    assert_eq!(stats.loss_percent(), 0.0);
}

#[test]
fn drop_everything_means_all_timeouts() {
    let (outcomes, stats) = run(start_server(1.0, Duration::ZERO), 3, ms(50));
    let expected: Vec<Outcome> = (0..3).map(|seq| Outcome::Timeout { seq }).collect();
    assert_eq!(outcomes, expected);
    assert_eq!(stats.received, 0);
    assert_eq!(stats.loss_percent(), 100.0);
}

#[test]
fn delay_shows_up_in_the_rtt() {
    let (outcomes, _) = run(start_server(0.0, ms(30)), 1, ms(1000));
    let [Outcome::Reply { rtt, .. }] = outcomes[..] else {
        panic!("expected one reply, got {outcomes:?}");
    };
    assert!(rtt >= ms(30), "rtt {rtt:?}");
}

#[test]
fn server_echoes_any_datagram_like_nc() {
    let server = start_server(0.0, Duration::ZERO);
    let client = client_socket();
    client.set_read_timeout(Some(ms(200))).unwrap();
    let mut buf = [0u8; 2048];

    client.send_to(b"hello\n", server).unwrap();
    let (n, from) = client.recv_from(&mut buf).unwrap();
    assert_eq!((&buf[..n], from), (&b"hello\n"[..], server));

    // Too big: ignored, so the next thing back is the echo of the 1500.
    client.send_to(&[b'x'; MAX_DATAGRAM + 1], server).unwrap();
    client.send_to(&[b'y'; MAX_DATAGRAM], server).unwrap();
    let (n, _) = client.recv_from(&mut buf).unwrap();
    assert_eq!(&buf[..n], &[b'y'; MAX_DATAGRAM][..]);
}

#[test]
fn replies_with_the_wrong_seq_or_source_are_ignored() {
    // A scripted fake server, so we control exactly what comes back.
    let fake = UdpSocket::bind("127.0.0.1:0").unwrap();
    let server = fake.local_addr().unwrap();
    thread::spawn(move || {
        let mut buf = [0u8; 64];
        let mut next_probe = || {
            let (n, peer) = fake.recv_from(&mut buf).unwrap();
            (Probe::decode(&buf[..n]).unwrap(), peer)
        };
        // Probe 0: answer only with a different seq.
        let (p0, client) = next_probe();
        fake.send_to(&Probe { seq: 99, ..p0 }.encode(), client)
            .unwrap();
        // Probe 1: the right bytes, but from the wrong address.
        let (p1, client) = next_probe();
        let imposter = UdpSocket::bind("127.0.0.1:0").unwrap();
        imposter.send_to(&p1.encode(), client).unwrap();
        // Probe 2: a stale reply to probe 0, some junk, then the real one.
        let (p2, client) = next_probe();
        fake.send_to(&p0.encode(), client).unwrap();
        fake.send_to(b"junk", client).unwrap();
        fake.send_to(&p2.encode(), client).unwrap();
    });

    let (outcomes, stats) = run(server, 3, ms(80));
    assert_eq!(outcomes[0], Outcome::Timeout { seq: 0 });
    assert_eq!(outcomes[1], Outcome::Timeout { seq: 1 });
    assert!(
        matches!(outcomes[2], Outcome::Reply { seq: 2, .. }),
        "{outcomes:?}"
    );
    assert_eq!((stats.sent, stats.received), (3, 1));
}
