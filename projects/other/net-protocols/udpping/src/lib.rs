//! udpping — `ping` over UDP, plus an echo server that can make the
//! network worse on purpose.
//!
//! UDP sends *datagrams*. Each `send_to` arrives whole or not at all, so
//! message boundaries survive (unlike TCP's byte stream). Nothing else is
//! promised: a datagram can be lost, duplicated, delayed, or reordered, and
//! nobody tells you. A ping client has to cope with all of that itself.
//!
//! # The probe
//!
//! The client sends 12-byte probes, big-endian:
//!
//! ```text
//!  0               4                                  12
//!  +---------------+----------------------------------+
//!  | seq: u32 BE   | sent_at: u64 BE (nanoseconds)    |
//!  +---------------+----------------------------------+
//! ```
//!
//! `sent_at` is measured from when the client started, on the monotonic
//! clock (`Instant`), so it can't jump when the wall clock is adjusted. The
//! server echoes the datagram unchanged, and the client gets the round-trip
//! time from the reply alone: `now - sent_at`. Real `ping` does the same
//! with a timestamp in the ICMP payload.
//!
//! Since the server never looks inside, it echoes *any* datagram of up to
//! 1500 bytes, so `nc -u` works as a client too.
//!
//! # Matching replies
//!
//! The client sends a probe, then waits up to a timeout for the reply with
//! *that* seq. Anything else that turns up in the meantime gets ignored:
//! a late reply to an earlier probe, a duplicate, junk, or a datagram from
//! some other address. Then it sends the next probe, one interval after the
//! previous one.
//!
//! # Making it worse
//!
//! `--drop P` makes the server ignore each datagram with probability P,
//! decided by a small xorshift PRNG. `--delay MS` makes it sleep before
//! echoing.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const PROBE_LEN: usize = 12;
/// Largest datagram the server echoes: a typical Ethernet MTU.
pub const MAX_DATAGRAM: usize = 1500;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub seq: u32,
    pub sent_at_nanos: u64,
}

impl Probe {
    pub fn encode(&self) -> [u8; PROBE_LEN] {
        let mut out = [0u8; PROBE_LEN];
        out[..4].copy_from_slice(&self.seq.to_be_bytes());
        out[4..].copy_from_slice(&self.sent_at_nanos.to_be_bytes());
        out
    }

    /// `None` unless `buf` is exactly one probe. A UDP reply is all or
    /// nothing, so a wrong length means it isn't ours.
    pub fn decode(buf: &[u8]) -> Option<Probe> {
        if buf.len() != PROBE_LEN {
            return None;
        }
        // `try_into` turns a slice into a fixed-size array, checking the
        // length; it can't fail here, and `?` covers it anyway.
        let seq = u32::from_be_bytes(buf[..4].try_into().ok()?);
        let sent_at_nanos = u64::from_be_bytes(buf[4..].try_into().ok()?);
        Some(Probe { seq, sent_at_nanos })
    }
}

/// Running totals for the summary line. RTTs aren't stored, just min, max,
/// and their sum, so memory doesn't grow with `-c`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub sent: u32,
    pub received: u32,
    pub min: Option<Duration>,
    pub max: Option<Duration>,
    pub total: Duration,
}

impl Stats {
    pub fn record_reply(&mut self, rtt: Duration) {
        self.received += 1;
        self.total += rtt;
        self.min = Some(self.min.map_or(rtt, |m| m.min(rtt)));
        self.max = Some(self.max.map_or(rtt, |m| m.max(rtt)));
    }

    pub fn loss_percent(&self) -> f64 {
        if self.sent == 0 {
            return 0.0;
        }
        let lost = self.sent.saturating_sub(self.received);
        100.0 * f64::from(lost) / f64::from(self.sent)
    }

    pub fn avg(&self) -> Option<Duration> {
        // `then` runs the closure only when received > 0: no divide by zero.
        (self.received > 0).then(|| self.total / self.received)
    }

    /// The last lines of a ping run, like the real `ping` prints.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "{} probes sent, {} received, {:.1}% loss",
            self.sent,
            self.received,
            self.loss_percent()
        );
        if let (Some(min), Some(avg), Some(max)) = (self.min, self.avg(), self.max) {
            out += &format!("\nrtt min/avg/max = {}/{}/{} ms", ms(min), ms(avg), ms(max));
        }
        out
    }
}

/// Milliseconds with three decimals, the way `ping` shows them.
pub fn ms(d: Duration) -> String {
    format!("{:.3}", d.as_secs_f64() * 1000.0)
}

/// xorshift64 (Marsaglia, 2003): three shifts and XORs give a fast
/// sequence that looks random enough for "drop this packet?". It's not for
/// anything secret: the next number is easy to predict from the last one.
pub struct XorShift(u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        // All-zero is the one state it can never leave (0 ^ 0 << n == 0).
        XorShift(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// A different seed every run, so `--drop` drops different packets.
    pub fn from_clock() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        XorShift::new(nanos as u64) // keep the fast-changing low 64 bits
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1): the top 53 bits, which is all the precision an
    /// f64 mantissa holds.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// True with probability `p`. Since next_f64 < 1.0, p = 1.0 is always
    /// true and p = 0.0 never is.
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }
}

pub struct ServerConfig {
    /// Probability of ignoring a datagram, 0.0 to 1.0.
    pub drop: f64,
    pub delay: Duration,
    pub seed: u64,
}

/// Echo loop. UDP has no connections, so there's no thread per client:
/// one socket, one loop, and the peer address comes with each datagram.
pub fn serve(socket: UdpSocket, config: ServerConfig) -> io::Result<()> {
    let mut rng = XorShift::new(config.seed);
    // One byte bigger than we accept: a datagram longer than the buffer is
    // silently cut short, so filling all 1501 bytes means "too big".
    let mut buf = [0u8; MAX_DATAGRAM + 1];
    loop {
        let (n, peer) = socket.recv_from(&mut buf)?;
        if n > MAX_DATAGRAM {
            eprintln!("[{peer}] datagram over {MAX_DATAGRAM} bytes, ignored");
            continue;
        }
        if rng.chance(config.drop) {
            eprintln!("[{peer}] {n} bytes, dropped");
            continue;
        }
        // Sleeping here holds up every other datagram too. That's fine
        // for one pinging client; see the rewrite exercises.
        if !config.delay.is_zero() {
            thread::sleep(config.delay);
        }
        // A failed send only affects this one peer, so log it and go on.
        match socket.send_to(&buf[..n], peer) {
            Ok(_) => eprintln!("[{peer}] {n} bytes, echoed"),
            Err(e) => eprintln!("[{peer}] send failed: {e}"),
        }
    }
}

pub struct PingConfig {
    pub count: u32,
    pub interval: Duration,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Reply { seq: u32, rtt: Duration },
    Timeout { seq: u32 },
}

/// Sends `config.count` probes to `server`, calling `report` as each one
/// is answered or times out, and returns the totals. Taking `impl FnMut`
/// lets the binary print as it goes while the tests collect into a Vec.
pub fn ping(
    socket: &UdpSocket,
    server: SocketAddr,
    config: &PingConfig,
    mut report: impl FnMut(Outcome),
) -> io::Result<Stats> {
    let start = Instant::now();
    let mut stats = Stats::default();
    for seq in 0..config.count {
        let sent_at = start.elapsed();
        // u64 nanoseconds last 584 years, so this `as` can't truncate.
        let probe = Probe {
            seq,
            sent_at_nanos: sent_at.as_nanos() as u64,
        };
        socket.send_to(&probe.encode(), server)?;
        stats.sent += 1;

        let outcome = match wait_for_reply(socket, server, seq, start, sent_at + config.timeout)? {
            Some(rtt) => {
                stats.record_reply(rtt);
                Outcome::Reply { seq, rtt }
            }
            None => Outcome::Timeout { seq },
        };
        report(outcome);

        // Keep a steady rhythm: the next probe leaves one interval after
        // this one did, however long the wait took. (`&& let` is a "let
        // chain", new in edition 2024.)
        if seq + 1 < config.count
            && let Some(pause) = (sent_at + config.interval).checked_sub(start.elapsed())
        {
            thread::sleep(pause);
        }
    }
    Ok(stats)
}

/// Waits until `deadline` (time since `start`) for the reply to probe
/// `seq`. Returns its RTT, or `None` on timeout.
fn wait_for_reply(
    socket: &UdpSocket,
    server: SocketAddr,
    seq: u32,
    start: Instant,
    deadline: Duration,
) -> io::Result<Option<Duration>> {
    let mut buf = [0u8; MAX_DATAGRAM];
    loop {
        // Recompute what's left on every pass, so ignored datagrams can't
        // stretch the wait. (A zero timeout is an error, hence the filter.)
        let left = deadline
            .checked_sub(start.elapsed())
            .filter(|d| !d.is_zero());
        let Some(left) = left else { return Ok(None) };
        socket.set_read_timeout(Some(left))?;

        let (n, from) = match socket.recv_from(&mut buf) {
            Ok(received) => received,
            Err(e) if is_timeout(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        match Probe::decode(&buf[..n]) {
            Some(probe) if from == server && probe.seq == seq => {
                let sent = Duration::from_nanos(probe.sent_at_nanos);
                return Ok(Some(start.elapsed().saturating_sub(sent)));
            }
            _ => continue, // late, duplicate, junk, or not from the server
        }
    }
}

/// An expired read timeout: Unix reports it as WouldBlock, Windows as
/// TimedOut.
fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_round_trip_and_layout() {
        let probe = Probe {
            seq: 3,
            sent_at_nanos: 0x0102_0304_0506_0708,
        };
        let bytes = probe.encode();
        assert_eq!(bytes, [0, 0, 0, 3, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(Probe::decode(&bytes), Some(probe));
    }

    #[test]
    fn wrong_length_is_not_a_probe() {
        let bytes = Probe {
            seq: 1,
            sent_at_nanos: 2,
        }
        .encode();
        assert_eq!(Probe::decode(&bytes[..11]), None);
        assert_eq!(Probe::decode(&[&bytes[..], &[0]].concat()), None);
        assert_eq!(Probe::decode(b""), None);
        assert_eq!(Probe::decode(b"hello\n"), None);
    }

    #[test]
    fn stats_math() {
        let mut stats = Stats {
            sent: 4,
            ..Stats::default()
        };
        for micros in [300, 100, 200] {
            stats.record_reply(Duration::from_micros(micros));
        }
        assert_eq!(stats.min, Some(Duration::from_micros(100)));
        assert_eq!(stats.max, Some(Duration::from_micros(300)));
        assert_eq!(stats.avg(), Some(Duration::from_micros(200)));
        assert_eq!(stats.loss_percent(), 25.0);
        assert_eq!(
            stats.summary(),
            "4 probes sent, 3 received, 25.0% loss\nrtt min/avg/max = 0.100/0.200/0.300 ms"
        );
    }

    #[test]
    fn stats_with_nothing_received() {
        let none = Stats::default();
        assert_eq!(none.loss_percent(), 0.0); // nothing sent: no divide by zero
        assert_eq!(none.avg(), None);
        let all_lost = Stats {
            sent: 5,
            ..Stats::default()
        };
        assert_eq!(all_lost.loss_percent(), 100.0);
        assert_eq!(all_lost.summary(), "5 probes sent, 0 received, 100.0% loss");
    }

    #[test]
    fn xorshift_is_deterministic_and_in_range() {
        let (mut a, mut b) = (XorShift::new(42), XorShift::new(42));
        for _ in 0..1000 {
            let x = a.next_f64();
            assert_eq!(x, b.next_f64());
            assert!((0.0..1.0).contains(&x));
        }
        // A zero seed would be stuck at zero forever; `new` avoids it.
        assert_ne!(XorShift::new(0).next_u64(), 0);
    }

    #[test]
    fn chance_matches_the_probability() {
        let mut rng = XorShift::new(7);
        assert!((0..1000).all(|_| rng.chance(1.0)));
        assert!((0..1000).all(|_| !rng.chance(0.0)));
        let hits = (0..10_000).filter(|_| rng.chance(0.3)).count();
        assert!((2_800..3_200).contains(&hits), "{hits} hits");
    }
}
