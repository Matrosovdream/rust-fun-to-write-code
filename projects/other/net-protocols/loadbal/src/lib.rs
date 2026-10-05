//! loadbal — a layer-4 (TCP) load balancer with health checks.
//!
//! ```text
//!                  ┌──────────────── loadbal ────────────────┐
//!   client ──────▶ │ :9400  accept → pick → connect → splice │ ──────▶ backend :9401
//!                  │                                         │         backend :9402
//!                  │ health thread: probe all every 2 s      │ ─probe─▶ backend :9403
//!   curl ────────▶ │ :9490  admin: plain-text stats table    │
//!                  └─────────────────────────────────────────┘
//! ```
//!
//! "Layer 4" means we forward TCP bytes without ever parsing them. Compare
//! with httpproxy (layer 7), which reads the request to decide where to go.
//! Here the decision is made before a single byte is read, so the same
//! balancer works for HTTP, Redis, SSH, or anything else that runs over TCP.
//!
//! For each accepted client:
//!   1. `pick` chooses a healthy backend (round-robin or least-connections).
//!   2. We connect with a timeout. If that fails, the backend is marked down
//!      and we pick again (failover), so the client never notices.
//!   3. `splice` copies bytes both ways until both sides have closed.
//!
//! Threads: one per client connection (plus one helper per connection for
//! the second copy direction), one health-check thread, and one admin
//! thread. They all share one `Arc<Pool>`.
//!
//! ## Atomics and memory ordering
//!
//! Every field that changes after startup is an atomic, so the pool can be
//! shared as a plain `&Pool` with no `Mutex`. All of them use
//! `Ordering::Relaxed`, and that is a deliberate choice:
//!
//! - Ordering is about what *other* memory a thread is guaranteed to see
//!   once it sees an atomic's new value. Acquire/Release pairs matter when an
//!   atomic *publishes* other data ("I wrote the buffer, then set ready =
//!   true"). None of ours publish anything: each one is the whole story.
//! - Atomicity is a separate guarantee. `fetch_add` is a single indivisible
//!   read-modify-write under *every* ordering, so two threads can never hand
//!   out the same round-robin ticket or lose a counter increment.
//! - The cost of Relaxed is staleness, which we can live with. A thread may
//!   briefly see an old `healthy` value: then it either tries a backend that
//!   just died (and fails over) or skips one that just came back (for one
//!   connection).
//!
//! If we ever stored a "reason it went down" string next to the flag, the
//! writer would need `Release` and the reader `Acquire`, so that seeing
//! `healthy == false` also guaranteed seeing the reason.

mod http;

use std::fmt::Write as _;
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

pub use http::{serve_admin, serve_hello};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    RoundRobin,
    LeastConn,
}

impl FromStr for Strategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "round-robin" => Ok(Strategy::RoundRobin),
            "least-conn" => Ok(Strategy::LeastConn),
            other => Err(format!("unknown strategy: {other}")),
        }
    }
}

/// Every knob a test might want to shrink lives here.
#[derive(Debug, Clone)]
pub struct Config {
    pub strategy: Strategy,
    pub connect_timeout: Duration,
    pub health_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            strategy: Strategy::RoundRobin,
            connect_timeout: Duration::from_secs(1),
            health_interval: Duration::from_secs(2),
        }
    }
}

/// A frozen, plain-data view of one backend. `pick` works on these, so it can
/// be unit-tested without atomics or sockets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub healthy: bool,
    pub active: usize,
}

/// The strategy as a pure function: which backend gets the next connection?
///
/// `turn` is a ticket that grows by one per decision. Round-robin uses it to
/// rotate over the healthy backends; least-conn uses it to break ties, so
/// that one-at-a-time requests (all with 0 active) still spread out.
pub fn pick(strategy: Strategy, backends: &[Snapshot], turn: usize) -> Option<usize> {
    // `filter` is lazy and the iterator is `Clone`, so we can walk it twice.
    let healthy = backends.iter().enumerate().filter(|(_, b)| b.healthy);
    let candidates: Vec<usize> = match strategy {
        Strategy::RoundRobin => healthy.map(|(i, _)| i).collect(),
        Strategy::LeastConn => {
            let fewest = healthy.clone().map(|(_, b)| b.active).min()?;
            healthy
                .filter(|(_, b)| b.active == fewest)
                .map(|(i, _)| i)
                .collect()
        }
    };
    if candidates.is_empty() {
        return None;
    }
    Some(candidates[turn % candidates.len()])
}

/// One upstream server and its live counters. Everything that changes is an
/// atomic, so it can be updated through a shared `&Backend`.
#[derive(Debug)]
pub struct Backend {
    pub addr: SocketAddr,
    healthy: AtomicBool,
    active: AtomicUsize,
    total: AtomicUsize,
}

impl Backend {
    /// Backends start healthy: the first health check, which runs right
    /// away, corrects that within one connect timeout.
    pub fn new(addr: SocketAddr) -> Self {
        Backend {
            addr,
            healthy: AtomicBool::new(true),
            active: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Sets the health flag and returns `true` if this call changed it.
    ///
    /// `swap` reads the old value and writes the new one in one atomic step.
    /// When the accept path and the health thread race to mark the same
    /// backend down, exactly one of them sees `true -> false`, so the change
    /// is logged once. A separate `load` then `store` could log it twice.
    pub fn set_healthy(&self, up: bool) -> bool {
        self.healthy.swap(up, Ordering::Relaxed) != up
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            healthy: self.is_healthy(),
            active: self.active(),
        }
    }

    /// Counts one proxied connection. The returned guard un-counts it when
    /// dropped, so `active` goes back down on every exit path, early returns
    /// and panics included (RAII).
    pub fn track(&self) -> ActiveGuard<'_> {
        self.active.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);
        ActiveGuard(self)
    }
}

pub struct ActiveGuard<'a>(&'a Backend);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// All backends plus the shared round-robin ticket counter.
#[derive(Debug)]
pub struct Pool {
    backends: Vec<Backend>,
    config: Config,
    /// `fetch_add` wraps around on overflow instead of panicking, and
    /// `turn % n` doesn't care, so this can run forever.
    turn: AtomicUsize,
}

impl Pool {
    pub fn new(addrs: Vec<SocketAddr>, config: Config) -> Self {
        Pool {
            backends: addrs.into_iter().map(Backend::new).collect(),
            config,
            turn: AtomicUsize::new(0),
        }
    }

    pub fn backends(&self) -> &[Backend] {
        &self.backends
    }

    /// Picks a backend and connects to it, failing over on errors.
    ///
    /// A failed connect marks that backend down, which removes it from the
    /// next `pick`, so `len` attempts are enough to try each backend once.
    pub fn connect(&self) -> Option<(&Backend, TcpStream)> {
        for _ in 0..self.backends.len() {
            let snapshots: Vec<Snapshot> = self.backends.iter().map(Backend::snapshot).collect();
            let turn = self.turn.fetch_add(1, Ordering::Relaxed);
            let backend = &self.backends[pick(self.config.strategy, &snapshots, turn)?];
            match TcpStream::connect_timeout(&backend.addr, self.config.connect_timeout) {
                Ok(stream) => return Some((backend, stream)),
                Err(e) => {
                    if backend.set_healthy(false) {
                        eprintln!("backend {} is down: {e}", backend.addr);
                    }
                }
            }
        }
        None
    }

    /// One round of health checks: a backend is up if it accepts a TCP
    /// connection within the timeout. Only changes are logged.
    pub fn check_health(&self) {
        for backend in &self.backends {
            let up = TcpStream::connect_timeout(&backend.addr, self.config.connect_timeout).is_ok();
            if backend.set_healthy(up) {
                let state = if up { "up" } else { "down" };
                eprintln!("backend {} is {state}", backend.addr);
            }
        }
    }

    /// The health thread's body: check, sleep, repeat, forever.
    pub fn run_health_checks(&self) {
        loop {
            self.check_health();
            thread::sleep(self.config.health_interval);
        }
    }

    /// The admin page. Each column is a separate atomic load, so a row is not
    /// a consistent snapshot (a connection may finish between two loads).
    /// For a monitoring page that's fine; for billing it wouldn't be.
    pub fn stats_table(&self) -> String {
        let mut out = format!(
            "{:<22} {:<5} {:>6} {:>6}\n",
            "backend", "state", "active", "total"
        );
        for b in &self.backends {
            let state = if b.is_healthy() { "up" } else { "down" };
            let addr = b.addr.to_string();
            // Writing to a String can't fail, hence the ignored Result.
            let _ = writeln!(
                out,
                "{addr:<22} {state:<5} {:>6} {:>6}",
                b.active(),
                b.total()
            );
        }
        out
    }
}

/// Accept loop: one thread per client. Runs until the listener fails.
pub fn serve(listener: TcpListener, pool: Arc<Pool>) {
    loop {
        match listener.accept() {
            Ok((client, peer)) => {
                let pool = Arc::clone(&pool);
                thread::spawn(move || handle(client, peer, &pool));
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

fn handle(client: TcpStream, peer: SocketAddr, pool: &Pool) {
    let Some((backend, upstream)) = pool.connect() else {
        // We can't send an error page: at layer 4 we don't know the protocol.
        eprintln!("[{peer}] no healthy backend, closing");
        return;
    };
    eprintln!("[{peer}] -> {}", backend.addr);
    let _active = backend.track();
    splice(&client, &upstream);
}

/// Copies bytes in both directions until both sides are done.
///
/// `&TcpStream` implements `Read` and `Write`, so two threads can share one
/// socket by reference, one reading and one writing. `thread::scope` lets the
/// helper thread borrow `client` and `upstream` from this stack frame,
/// because the scope joins it before returning.
pub fn splice(client: &TcpStream, upstream: &TcpStream) {
    thread::scope(|s| {
        s.spawn(|| pipe(client, upstream));
        pipe(upstream, client);
    });
}

/// One direction: copy until EOF, then pass the EOF on with a half-close
/// (`Shutdown::Write`), so the other side knows we're done sending but can
/// still reply. Errors (e.g. a reset) just end the copy: that's a normal way
/// for a proxied connection to end.
///
/// A client that never closes its side keeps its thread alive, as with any
/// plain TCP proxy. Real balancers add idle timeouts.
fn pipe(mut from: &TcpStream, mut to: &TcpStream) {
    let _ = io::copy(&mut from, &mut to);
    let _ = to.shutdown(Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(active: usize) -> Snapshot {
        Snapshot {
            healthy: true,
            active,
        }
    }

    fn down() -> Snapshot {
        Snapshot {
            healthy: false,
            active: 0,
        }
    }

    fn picks(strategy: Strategy, backends: &[Snapshot], turns: usize) -> Vec<usize> {
        (0..turns)
            .filter_map(|t| pick(strategy, backends, t))
            .collect()
    }

    #[test]
    fn round_robin_rotates() {
        let all_up = [up(0), up(0), up(0)];
        assert_eq!(picks(Strategy::RoundRobin, &all_up, 6), [0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn round_robin_skips_down_backends_evenly() {
        // The survivors share the load equally; neither gets a double turn.
        let middle_down = [up(0), down(), up(0)];
        assert_eq!(picks(Strategy::RoundRobin, &middle_down, 4), [0, 2, 0, 2]);
    }

    #[test]
    fn least_conn_prefers_fewest_active() {
        let busy = [up(5), up(1), up(3)];
        assert_eq!(picks(Strategy::LeastConn, &busy, 3), [1, 1, 1]);
        // A down backend is ignored even if it's idle.
        let idle_but_down = [up(2), down(), up(4)];
        assert_eq!(pick(Strategy::LeastConn, &idle_but_down, 0), Some(0));
    }

    #[test]
    fn least_conn_breaks_ties_by_turn() {
        let tied = [up(1), up(0), up(0)];
        assert_eq!(picks(Strategy::LeastConn, &tied, 4), [1, 2, 1, 2]);
    }

    #[test]
    fn nothing_healthy_means_no_pick() {
        for strategy in [Strategy::RoundRobin, Strategy::LeastConn] {
            assert_eq!(pick(strategy, &[down(), down()], 0), None);
            assert_eq!(pick(strategy, &[], 7), None);
        }
    }

    #[test]
    fn huge_turn_numbers_are_fine() {
        assert_eq!(
            pick(Strategy::RoundRobin, &[up(0), up(0)], usize::MAX),
            Some(1)
        );
    }

    #[test]
    fn strategy_parses() {
        assert_eq!("round-robin".parse(), Ok(Strategy::RoundRobin));
        assert_eq!("least-conn".parse(), Ok(Strategy::LeastConn));
        assert!("random".parse::<Strategy>().is_err());
    }

    #[test]
    fn health_transitions_are_reported_once() {
        let b = Backend::new("127.0.0.1:1".parse().unwrap());
        assert!(b.is_healthy());
        assert!(!b.set_healthy(true), "up -> up is not a change");
        assert!(b.set_healthy(false), "up -> down is a change");
        assert!(!b.set_healthy(false), "down -> down is not");
        assert!(b.set_healthy(true), "down -> up is");
        assert!(b.is_healthy());
    }

    #[test]
    fn track_counts_active_and_total() {
        let b = Backend::new("127.0.0.1:1".parse().unwrap());
        let first = b.track();
        let second = b.track();
        assert_eq!((b.active(), b.total()), (2, 2));
        drop(first);
        assert_eq!((b.active(), b.total()), (1, 2));
        drop(second);
        assert_eq!((b.active(), b.total()), (0, 2));
    }

    #[test]
    fn stats_table_lists_every_backend() {
        let addrs = vec![
            "127.0.0.1:9401".parse().unwrap(),
            "127.0.0.1:9402".parse().unwrap(),
        ];
        let pool = Pool::new(addrs, Config::default());
        pool.backends()[1].set_healthy(false);
        let _active = pool.backends()[0].track();
        let table = pool.stats_table();
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("backend"));
        assert_eq!(
            lines[1].split_whitespace().collect::<Vec<_>>(),
            ["127.0.0.1:9401", "up", "1", "1"]
        );
        assert_eq!(
            lines[2].split_whitespace().collect::<Vec<_>>(),
            ["127.0.0.1:9402", "down", "0", "0"]
        );
    }
}
