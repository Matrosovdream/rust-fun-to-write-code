//! Integration tests over real localhost sockets: real backends, a real
//! balancer, real TCP connections. Every port is picked by the OS.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use loadbal::{Config, Pool, Strategy, serve, serve_admin, serve_hello};

fn fast_config() -> Config {
    Config {
        strategy: Strategy::RoundRobin,
        connect_timeout: Duration::from_millis(200),
        health_interval: Duration::from_millis(50),
    }
}

/// Starts a hello backend on a free port.
fn start_backend() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve_hello(listener));
    addr
}

/// An address nobody is listening on: bind a free port, then let it go.
fn dead_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn start_balancer(backends: Vec<SocketAddr>, config: Config) -> (SocketAddr, Arc<Pool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let pool = Arc::new(Pool::new(backends, config));
    let server_pool = Arc::clone(&pool);
    thread::spawn(move || serve(listener, server_pool));
    (addr, pool)
}

/// One HTTP request on a fresh connection; returns the whole response.
fn get(addr: SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn served_by(response: &str) -> &str {
    response.trim_end().rsplit("hello from ").next().unwrap()
}

/// Polls until `cond` holds; fails the test after 2 s instead of hanging.
fn wait_until(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn round_robin_spreads_requests_evenly() {
    let backends = vec![start_backend(), start_backend(), start_backend()];
    let (lb, pool) = start_balancer(backends.clone(), fast_config());

    let served: Vec<String> = (0..6).map(|_| served_by(&get(lb)).to_string()).collect();
    let expected: Vec<String> = backends
        .iter()
        .chain(&backends)
        .map(|a| a.to_string())
        .collect();
    assert_eq!(served, expected);

    // Each backend served two connections, and none is still active.
    // `active` drops a moment after the client sees EOF, so poll for it.
    for b in pool.backends() {
        assert_eq!(b.total(), 2);
        wait_until("connections to finish", || b.active() == 0);
    }
}

#[test]
fn failover_skips_a_dead_backend() {
    let live = [start_backend(), start_backend()];
    let dead = dead_addr();
    let (lb, pool) = start_balancer(vec![dead, live[0], live[1]], fast_config());

    for _ in 0..4 {
        let response = get(lb);
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert_ne!(served_by(&response), dead.to_string());
    }
    let dead_backend = &pool.backends()[0];
    assert!(!dead_backend.is_healthy());
    assert_eq!(dead_backend.total(), 0);
}

#[test]
fn no_healthy_backend_closes_the_connection() {
    let (lb, _pool) = start_balancer(vec![dead_addr(), dead_addr()], fast_config());
    // Send nothing: if we had sent a request, the balancer would close with
    // our bytes unread, and the kernel would turn that into a reset (RST).
    let mut stream = TcpStream::connect(lb).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut rest = Vec::new();
    assert_eq!(stream.read_to_end(&mut rest).unwrap(), 0);
}

#[test]
fn health_checker_drops_and_readmits_a_backend() {
    let addr = dead_addr();
    let pool = Arc::new(Pool::new(vec![addr], fast_config()));
    let checker = Arc::clone(&pool);
    thread::spawn(move || checker.run_health_checks());

    wait_until("backend marked down", || !pool.backends()[0].is_healthy());

    // "Restart" the backend on the same port: the next check notices.
    let listener = TcpListener::bind(addr).unwrap();
    thread::spawn(move || serve_hello(listener));
    wait_until("backend marked up", || pool.backends()[0].is_healthy());
}

#[test]
fn admin_endpoint_serves_the_stats_table() {
    let backend = start_backend();
    let (lb, pool) = start_balancer(vec![backend, dead_addr()], fast_config());
    get(lb);
    pool.check_health();

    let admin = TcpListener::bind("127.0.0.1:0").unwrap();
    let admin_addr = admin.local_addr().unwrap();
    thread::spawn(move || serve_admin(admin, pool));

    let response = get(admin_addr);
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("Connection: close\r\n"));
    let (_, body) = response.split_once("\r\n\r\n").unwrap();
    let rows: Vec<Vec<&str>> = body
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(rows[0], ["backend", "state", "active", "total"]);
    assert_eq!(rows[1], [backend.to_string().as_str(), "up", "0", "1"]);
    assert_eq!(rows[2][1], "down");
}
